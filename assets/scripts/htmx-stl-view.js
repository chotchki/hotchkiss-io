import * as THREE from 'three';
import Stats from 'three/addons/libs/stats.module.js';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';

//Using the code from the STL Loader example here: https://threejs.org/examples/?q=stl#webgl_loader_stl
function addShadowedLight(scene, x, y, z, color, intensity) {

    const directionalLight = new THREE.DirectionalLight(color, intensity);
    directionalLight.position.set(x, y, z);
    scene.add(directionalLight);

    directionalLight.castShadow = true;

    const d = 1;
    directionalLight.shadow.camera.left = - d;
    directionalLight.shadow.camera.right = d;
    directionalLight.shadow.camera.top = d;
    directionalLight.shadow.camera.bottom = - d;

    directionalLight.shadow.camera.near = 1;
    directionalLight.shadow.camera.far = 4;

    directionalLight.shadow.bias = - 0.002;

}

htmx.onLoad(function (content) {
    var stl_objects = content.querySelectorAll(".stl-view");

    for (const stl_o of stl_objects) {
        const stl_url = stl_o.attributes['src'].nodeValue;

        let camera = new THREE.PerspectiveCamera(35, stl_o.clientWidth / stl_o.clientHeight, 1, 15);
        camera.position.set(3, 0.15, 3);

        let cameraTarget = new THREE.Vector3(0, - 0.25, 0);

        let scene = new THREE.Scene();
        scene.background = new THREE.Color(0x72645b);
        //scene.fog = new THREE.Fog(0x72645b, 2, 15);

        // Ground
        //const plane = new THREE.Mesh(
        //    new THREE.PlaneGeometry(40, 40),
        //    new THREE.MeshPhongMaterial({ color: 0xcbcbcb, specular: 0x474747 })
        //);
        //plane.rotation.x = - Math.PI / 2;
        //plane.position.y = - 0.5;
        //scene.add(plane);

        //plane.receiveShadow = true;

        // ASCII file

        const loader = new STLLoader();
        loader.load(stl_url, function (geometry) {

            const material = new THREE.MeshPhongMaterial({ color: 0xff9c7c, specular: 0x494949, shininess: 200 });
            const mesh = new THREE.Mesh(geometry, material);

            //mesh.position.set(0.5, 0.2, 0);
            mesh.position.set(0, - 0.25, 0.6);
            mesh.rotation.set(0, - Math.PI / 2, 0);
            mesh.scale.set(0.05, 0.05, 0.05);

            mesh.castShadow = true;
            mesh.receiveShadow = true;

            scene.add(mesh);

        });

        // Lights

        scene.add(new THREE.HemisphereLight(0x8d7c7c, 0x494966, 3));

        addShadowedLight(scene, 1, 1, 1, 0xffffff, 3.5);
        addShadowedLight(scene, 0.5, 1, - 1, 0xffd500, 3);

        // renderer
        let renderer = new THREE.WebGLRenderer({ antialias: true });
        renderer.setPixelRatio(window.devicePixelRatio);
        renderer.setSize(stl_o.clientWidth, stl_o.clientHeight);

        renderer.shadowMap.enabled = true;

        stl_o.appendChild(renderer.domElement);

        let controls = new OrbitControls(camera, renderer.domElement);
        controls.addEventListener('change', () => {
            renderer.render(scene, camera);
        });

        camera.lookAt(cameraTarget);

        renderer.render(scene, camera);
    }



});