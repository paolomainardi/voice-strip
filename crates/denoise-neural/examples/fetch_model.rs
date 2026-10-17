//! Downloads the DeepFilterNet model into the user's cache, as the app does
//! once the user agrees to it, and prints where it is (`just fetch-model`).

use denoise_neural::model;

fn main() {
    let result = match model::locate() {
        Ok(path) => Ok(path),
        Err(_) => model::Download::start().and_then(model::Download::wait),
    };
    match result {
        Ok(path) => println!("{}", path.display()),
        Err(e) => {
            eprintln!("fetch-model: {e}");
            std::process::exit(1);
        }
    }
}
