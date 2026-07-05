//! Application shell for the vector3d engine: window, event loop, input,
//! camera controllers.

pub mod collide;
mod fly_camera;
mod fps_controller;
mod input;
mod orbit_camera;
pub mod scene;
mod shell;

pub use collide::TriangleSoup;
pub use fly_camera::FlyCamera;
pub use fps_controller::FpsController;
pub use input::Input;
pub use orbit_camera::OrbitCamera;
pub use scene::{BakedScene, load_scene, load_scene_from_str};
pub use shell::{App, run};
pub use winit::event::MouseButton;
pub use winit::keyboard::KeyCode;
