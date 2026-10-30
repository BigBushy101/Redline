//! Embeds the Redline icon and version info in the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/redline.ico");
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/redline.ico");
        res.set("ProductName", "Redline");
        res.set("FileDescription", "Redline");
        res.set("OriginalFilename", "redline.exe");
        let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
        let mut parts = ver.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        let major = parts.next().unwrap_or(0) & 0xffff;
        let minor = parts.next().unwrap_or(0) & 0xffff;
        let patch = parts.next().unwrap_or(0) & 0xffff;
        let version = (major << 48) | (minor << 32) | (patch << 16);
        res.set_version_info(winres::VersionInfo::FILEVERSION, version);
        res.set_version_info(winres::VersionInfo::PRODUCTVERSION, version);
        if let Err(e) = res.compile() {
            panic!("couldn't compile the Windows icon resource: {e}");
        }
    }
}
