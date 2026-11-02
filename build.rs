fn main() {
    // Rebuild when a translation changes, not only when a .slint file does.
    println!("cargo:rerun-if-changed=lang");
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new()
            .with_bundled_translations("lang")
            // The .po files carry no msgctxt, so strings match by text alone.
            .with_default_translation_context(slint_build::DefaultTranslationContext::None),
    )
    .expect("Slint compilation failed");
}
