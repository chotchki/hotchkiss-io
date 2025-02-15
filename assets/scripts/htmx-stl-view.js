import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';

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
        elem.innerHTML = '';
        //if (!THREE.WEBGL.isWebGLAvailable()) {
        //    elem.appendChild(THREE.WEBGL.getWebGLErrorMessage()); // FIXME: own (styled) message
        //    return;
        //}

        var renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
        var camera = new THREE.PerspectiveCamera(50, elem.clientWidth / elem.clientHeight, 0.1, 1000);

        renderer.setSize(elem.clientWidth, elem.clientHeight);
        elem.appendChild(renderer.domElement);

        window.addEventListener('resize', function () {
            renderer.setSize(elem.clientWidth, elem.clientHeight);
            camera.aspect = elem.clientWidth / elem.clientHeight;
            camera.updateProjectionMatrix();
        }, false);

        var controls = new OrbitControls(camera, renderer.domElement);
        controls.enableDamping = true;
        controls.rotateSpeed = 0.5;
        controls.dampingFactor = 0.25;
        controls.enableZoom = true;
        controls.enablePan = false;
        controls.autoRotate = true;

        var scene = new THREE.Scene();

        // Setup lights (dependent on camera); stolen from viewstl
        scene.add(camera);
        camera.add(new THREE.AmbientLight(0x202020));
        let dl = new THREE.DirectionalLight(0xffffff, 0.75);
        dl.position.x = 1;
        dl.position.y = 1;
        dl.position.z = 2;
        dl.position.normalize();
        camera.add(dl);
        let pl = new THREE.PointLight(0xffffff, 0.3);
        pl.position.x = 0;
        pl.position.y = -25;
        pl.position.z = 10;
        pl.position.normalize();
        camera.add(pl);

        (new STLLoader()).load(data['filename'], function (geometry) {
            // Determine the color
            var colorString = data['color'];
            if (colorString != null) { var color = new THREE.Color(colorString); }
            else { var color = 0x909090 }

            // Set up the material
            var material = new THREE.MeshLambertMaterial({ color: color, wireframe: false, vertexColors: false });
            var mesh = new THREE.Mesh(geometry, material);
            scene.add(mesh);

            // Compute the middle
            var middle = new THREE.Vector3();
            geometry.computeBoundingBox();
            geometry.boundingBox.getCenter(middle);

            // Center it
            mesh.geometry.applyMatrix4(new THREE.Matrix4().makeTranslation(-middle.x, -middle.y, -middle.z));

            // Rotate, if desired
            var to_rad = Math.PI / 180;
            mesh.rotation.x = to_rad * (data['rotationx'] || 0);
            mesh.rotation.y = to_rad * (data['rotationy'] || 0);
            mesh.rotation.z = to_rad * (data['rotationz'] || 0);

            var helper = null;
            if (data['showbb']) {
                // Show bounding box, if desired
                var helper = new THREE.BoxHelper(mesh);
                helper.material.color.set(0xbbddff);
                scene.add(helper);
            }

            // Pull the camera away as needed
            fitCameraToCenteredObject(camera, mesh, data['camoffset'] || 1, controls);

            var animate = function () {
                requestAnimationFrame(animate);
                if (helper) {
                    helper.update();
                }
                controls.update();
                // console.log([data['filename'], JSON.stringify(camera.position)]);
                renderer.render(scene, camera);
            }; animate();

        });
    }

    const fitCameraToCenteredObject = function (camera, object, offset, orbitControls) {
        const boundingBox = new THREE.Box3();
        boundingBox.setFromObject(object);

        var middle = new THREE.Vector3();
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
        let dx = size.z / 2 + Math.abs(size.x / 2 / Math.tan(fovh / 2));
        let dy = size.z / 2 + Math.abs(size.y / 2 / Math.tan(fov / 2));
        let cameraZ = Math.max(dx, dy);

        // offset the camera, if desired (to avoid filling the whole canvas)
        if (offset !== undefined && offset !== 0) cameraZ *= offset;

        camera.position.set(0, 0, cameraZ);

        // set the far plane of the camera so that it easily encompasses the whole object
        const minZ = boundingBox.min.z;
        const cameraToFarEdge = (minZ < 0) ? -minZ + cameraZ : cameraZ - minZ;

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
            stl_o.innerHTML = '<p class="text-xl">Loading STL... <i class="fa-solid fa-spinner animate-spin"></i></p>';

            const stl_url = stl_o.dataset.filename;

            StlViewer(stl_o, { filename: stl_url });
        }
    });
})();