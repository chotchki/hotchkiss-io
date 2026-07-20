import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { ThreeMFLoader } from "three/addons/loaders/3MFLoader.js";
import { STLLoader } from "three/addons/loaders/STLLoader.js";

/*
 * From: https://wejn.org/2020/12/cracking-the-threejs-object-fitting-nut/
 * @author: Michal Jirku
 *
 * A small datatag handler to initialize custom StlViewer.
 *
 * This is inspired by:
 * - https://tonybox.net/posts/simple-stl-viewer/
 * - https://github.com/omrips/viewstl
 * - https://github.com/mrdoob/three.js/issues/6784#issuecomment-315968779
 * - https://themetalmuncher.github.io/fov-calc/
 * - http://chrisjones.id.au/FOV/fovtext.htm
 *
 * License: MIT.
 *
 */
(function () {
  function StlViewer(elem, data) {
    elem.innerHTML = "";
    //if (!THREE.WEBGL.isWebGLAvailable()) {
    //    elem.appendChild(THREE.WEBGL.getWebGLErrorMessage()); // FIXME: own (styled) message
    //    return;
    //}

    var renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    var camera = new THREE.PerspectiveCamera(
      50,
      elem.clientWidth / elem.clientHeight,
      0.1,
      1000,
    );

    renderer.setSize(elem.clientWidth, elem.clientHeight);
    elem.appendChild(renderer.domElement);

    function resize() {
      renderer.setSize(elem.clientWidth, elem.clientHeight);
      camera.aspect = elem.clientWidth / elem.clientHeight;
      camera.updateProjectionMatrix();
    }
    window.addEventListener("resize", resize, false);
    // Entering/leaving fullscreen resizes the element's box to/from the screen;
    // re-fit after the browser applies the fullscreen layout (a tick later).
    document.addEventListener("fullscreenchange", function () {
      setTimeout(resize, 60);
    });

    var controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.rotateSpeed = 0.5;
    controls.dampingFactor = 0.25;
    controls.enableZoom = true;
    controls.enablePan = false;
    // The attract spin: slow turntable, off entirely under reduced-motion.
    // autoRotateSpeed is per-second only when update() gets a delta (see clock).
    var reduceMotion =
      window.matchMedia &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    controls.autoRotate = !reduceMotion;
    controls.autoRotateSpeed = 1.0;
    // First grab kills the spin for good — it's an attract loop, not the
    // interaction model; auto-resume would wrestle the camera back.
    controls.addEventListener("start", function () {
      controls.autoRotate = false;
      syncAutorotateFlag();
    });
    // Observable seam for the browser e2e (module scope hides `controls`).
    function syncAutorotateFlag() {
      elem.dataset.autorotate = controls.autoRotate ? "on" : "off";
    }
    syncAutorotateFlag();

    // Without a delta, OrbitControls steps a fixed angle per update() call —
    // a 120Hz display spins twice as fast. The clock makes it wall-clock true.
    var clock = new THREE.Clock();

    var scene = new THREE.Scene();

    // Setup lights (dependent on camera); stolen from viewstl
    scene.add(camera);
    camera.add(new THREE.AmbientLight(0x202020));
    const dl = new THREE.DirectionalLight(0xffffff, 0.75);
    dl.position.x = 1;
    dl.position.y = 1;
    dl.position.z = 2;
    dl.position.normalize();
    camera.add(dl);
    const pl = new THREE.PointLight(0xffffff, 0.3);
    pl.position.x = 0;
    pl.position.y = -25;
    pl.position.z = 10;
    pl.position.normalize();
    camera.add(pl);

    var to_rad = Math.PI / 180;

    // Shared tail once a model object is in hand (STL Mesh or 3MF Group):
    // center at the origin, apply optional rotation, fit the camera, animate.
    function present(object) {
      var box = new THREE.Box3().setFromObject(object);
      var mid = new THREE.Vector3();
      box.getCenter(mid);
      object.position.set(-mid.x, -mid.y, -mid.z);
      object.rotation.x = to_rad * (data["rotationx"] || 0);
      object.rotation.y = to_rad * (data["rotationy"] || 0);
      object.rotation.z = to_rad * (data["rotationz"] || 0);
      scene.add(object);
      fitCameraToCenteredObject(
        camera,
        object,
        data["camoffset"] || 1,
        controls,
      );
      (function animate() {
        requestAnimationFrame(animate);
        controls.update(clock.getDelta());
        renderer.render(scene, camera);
      })();
    }

    // 3MF carries its OWN materials (colors) — load it as-is, don't recolor.
    if ((data["format"] || "").toLowerCase() === "3mf") {
      new ThreeMFLoader().load(data["filename"], function (object) {
        present(object);
      });
      return;
    }

    new STLLoader().load(data["filename"], function (geometry) {
      // Determine the color — default to the site's yellow (#ffc935); a
      // `data-color` on the <object> can override per-model.
      var colorString = data["color"];
      var color = colorString != null ? new THREE.Color(colorString) : 0xffc935;

      // Set up the material
      var material = new THREE.MeshLambertMaterial({
        color: color,
        wireframe: false,
        vertexColors: false,
      });
      var mesh = new THREE.Mesh(geometry, material);
      scene.add(mesh);

      // Compute the middle
      var middle = new THREE.Vector3();
      geometry.computeBoundingBox();
      geometry.boundingBox.getCenter(middle);

      // Center it
      mesh.geometry.applyMatrix4(
        new THREE.Matrix4().makeTranslation(-middle.x, -middle.y, -middle.z),
      );

      // Rotate, if desired
      var to_rad = Math.PI / 180;
      mesh.rotation.x = to_rad * (data["rotationx"] || 0);
      mesh.rotation.y = to_rad * (data["rotationy"] || 0);
      mesh.rotation.z = to_rad * (data["rotationz"] || 0);

      var helper = null;
      if (data["showbb"]) {
        // Show bounding box, if desired
        helper = new THREE.BoxHelper(mesh);
        helper.material.color.set(0xbbddff);
        scene.add(helper);
      }

      // Pull the camera away as needed
      fitCameraToCenteredObject(camera, mesh, data["camoffset"] || 1, controls);

      var animate = function () {
        requestAnimationFrame(animate);
        if (helper) {
          helper.update();
        }
        controls.update(clock.getDelta());
        // console.log([data['filename'], JSON.stringify(camera.position)]);
        renderer.render(scene, camera);
      };
      animate();
    });
  }

  const fitCameraToCenteredObject = function (
    camera,
    object,
    offset,
    orbitControls,
  ) {
    const boundingBox = new THREE.Box3();
    boundingBox.setFromObject(object);

    var size = new THREE.Vector3();
    boundingBox.getSize(size);

    // figure out how to fit the box in the view:
    // 1. figure out horizontal FOV (on non-1.0 aspects)
    // 2. figure out distance from the object in X and Y planes
    // 3. select the max distance (to fit both sides in)
    //
    // The reason is as follows:
    //
    // Imagine a bounding box (BB) is centered at (0,0,0).
    // Camera has vertical FOV (camera.fov) and horizontal FOV
    // (camera.fov scaled by aspect, see fovh below)
    //
    // Therefore if you want to put the entire object into the field of view,
    // you have to compute the distance as: z/2 (half of Z size of the BB
    // protruding towards us) plus for both X and Y size of BB you have to
    // figure out the distance created by the appropriate FOV.
    //
    // The FOV is always a triangle:
    //
    //  (size/2)
    // +--------+
    // |       /
    // |      /
    // |     /
    // | FÂ° /
    // |   /
    // |  /
    // | /
    // |/
    //
    // FÂ° is half of respective FOV, so to compute the distance (the length
    // of the straight line) one has to: `size/2 / Math.tan(F)`.
    //
    // FTR, from https://threejs.org/docs/#api/en/cameras/PerspectiveCamera
    // the camera.fov is the vertical FOV.

    const fov = camera.fov * (Math.PI / 180);
    const fovh = 2 * Math.atan(Math.tan(fov / 2) * camera.aspect);
    const dx = size.z / 2 + Math.abs(size.x / 2 / Math.tan(fovh / 2));
    const dy = size.z / 2 + Math.abs(size.y / 2 / Math.tan(fov / 2));
    let cameraZ = Math.max(dx, dy);

    // offset the camera, if desired (to avoid filling the whole canvas)
    if (offset !== undefined && offset !== 0) cameraZ *= offset;

    camera.position.set(0, 0, cameraZ);

    // set the far plane of the camera so that it easily encompasses the whole object
    const minZ = boundingBox.min.z;
    const cameraToFarEdge = minZ < 0 ? -minZ + cameraZ : cameraZ - minZ;

    camera.far = cameraToFarEdge * 3;
    camera.updateProjectionMatrix();

    if (orbitControls !== undefined) {
      // set camera to rotate around the center
      orbitControls.target = new THREE.Vector3(0, 0, 0);

      // prevent camera from zooming out far enough to create far plane cutoff
      orbitControls.maxDistance = cameraToFarEdge * 2;
    }
  };

  htmx.onLoad(function (content) {
    var stl_objects = content.querySelectorAll(".stl-view");

    for (const stl_o of stl_objects) {
      stl_o.innerHTML =
        '<p class="text-xl">Loading STL... <span class="stl-spinner"></span></p>';

      const stl_url = stl_o.dataset.filename;

      StlViewer(stl_o, {
        filename: stl_url,
        color: stl_o.dataset.color,
        format: stl_o.dataset.format,
      });

      // Fullscreen the .stl-embed WRAPPER (keeps the toggle button visible so
      // you can exit; Esc works too). three.js re-fits via fullscreenchange.
      const wrapper = stl_o.closest(".stl-embed");
      const fsBtn = wrapper && wrapper.querySelector(".stl-fullscreen");
      if (fsBtn) {
        fsBtn.addEventListener("click", function () {
          if (document.fullscreenElement) {
            document.exitFullscreen();
          } else if (wrapper.requestFullscreen) {
            wrapper.requestFullscreen().catch(function () {});
          }
        });
      }
    }
  });
})();
