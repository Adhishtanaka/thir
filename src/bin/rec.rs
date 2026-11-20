//! rec: the recorder, its own small program so that den can require exactly /bin/rec before it asks about the microphone (see gui/mic.rs). The code is src/apps/rec.rs.
fn main() { let args: Vec<String> = std::env::args().skip(1).collect(); std::process::exit(thir::apps::rec::run(&args)); }
