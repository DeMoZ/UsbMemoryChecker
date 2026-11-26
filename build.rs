// Icon and description of the .exe on Windows.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "UsbMemoryChecker");
        res.set("ProductName", "UsbMemoryChecker");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not added: {e}");
        }
    }
}
