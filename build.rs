#[cfg(windows)]
use winres::WindowsResource;

fn main() {
    #[cfg(windows)]
    {
        WindowsResource::new()
            .set_icon("img/Tomato-downloader-ico.ico")
            .compile()
            .expect("failed to embed Windows icon");
    }
}
