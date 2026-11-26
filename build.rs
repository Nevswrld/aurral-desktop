fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/aurral-icon.ico");
        if let Err(e) = res.compile() {
            eprintln!("winresource compile failed: {e}");
            std::process::exit(1);
        }
    }
}
