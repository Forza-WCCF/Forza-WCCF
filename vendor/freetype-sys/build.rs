// FreeType 2.13.3 built from freetype2/ (always: the `bundled` feature is accepted and changes nothing).
fn main() {
    let src = std::path::Path::new("freetype2/src");
    let files = [
        "autofit/autofit", "base/ftbase", "base/ftbbox", "base/ftbdf", "base/ftbitmap", "base/ftcid", "base/ftdebug",
        "base/ftfstype", "base/ftgasp", "base/ftglyph", "base/ftgxval", "base/ftinit", "base/ftmm", "base/ftotval",
        "base/ftpatent", "base/ftpfr", "base/ftstroke", "base/ftsynth", "base/ftsystem", "base/fttype1",
        "base/ftwinfnt", "psnames/psnames", "sfnt/sfnt", "smooth/smooth", "truetype/truetype",
    ];
    cc::Build::new()
        .warnings(false)
        .include(".")
        .include("freetype2/include")
        .define("FT2_BUILD_LIBRARY", None)
        .define("FT_CONFIG_MODULES_H", "<ftmodule.h>")
        .define("FT_CONFIG_OPTIONS_H", "<ftoption.h>")
        .files(files.iter().map(|f| src.join(f).with_extension("c")))
        .compile("freetype2");
    println!("cargo:rerun-if-changed=freetype2");
    println!("cargo:rerun-if-changed=ftmodule.h");
    println!("cargo:rerun-if-changed=ftoption.h");
}
