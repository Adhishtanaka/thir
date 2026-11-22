//! camera: the webcam app, its own small program so that den can require exactly /bin/camera before it asks about the camera (see gui/cam.rs). The code is src/apps/camera.rs.
fn main() { let args: Vec<String> = std::env::args().skip(1).collect(); std::process::exit(thir::apps::camera::run(&args)); }
