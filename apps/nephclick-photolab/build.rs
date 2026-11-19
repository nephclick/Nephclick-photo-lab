//! Windows only: embed the app icon and version info (VERSIONINFO) into `nephclick-photolab.exe`.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `NEPHCLICK_PHOTOLAB_REQUIRE_WINRES=1` (set by the
//! release workflow) turns it into an error.

/// Embeds the Windows resources and delay-loads the print-only system DLLs (#5884, #8272).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/nephclick-photolab.ico");
    println!("cargo:rerun-if-env-changed=NEPHCLICK_PHOTOLAB_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // Printing to PDF (winprint's `PdfCreateRenderer`) imports Windows.Data.Pdf.dll, and its
    // printer options (the Print Ticket API) import prntvpt.dll; stripped Windows images (NTLite,
    // some N/LTSC builds) lack them. A load-time import stops the exe from starting at all
    // ("windows.data.pdf.dll was not found", #5884; "prntvpt.dll", #8272), so load them on first
    // use; the print commands check for them first and report an error instead.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:windows.data.pdf.dll");
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:prntvpt.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/nephclick-photolab.ico")
        .set("ProductName", "Nephclick PhotoLab")
        .set("FileDescription", "Nephclick PhotoLab image editor")
        .set("CompanyName", "Learning Machines LLC")
        .set("LegalCopyright", "Copyright (c) the Nephclick PhotoLab authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "nephclick-photolab.exe")
        .set("InternalName", "nephclick-photolab");
    if let Err(e) = res.compile() {
        if std::env::var_os("NEPHCLICK_PHOTOLAB_REQUIRE_WINRES").is_some() {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=nephclick-photolab.exe built without icon/version resources: {e}");
    }
}
