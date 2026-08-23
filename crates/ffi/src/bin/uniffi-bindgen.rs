//! `cargo run -p sluice-ffi --features bindgen --bin uniffi-bindgen -- generate --library <libsluice_ffi.dylib> --language swift|kotlin --out-dir <dir>`
fn main() {
    uniffi::uniffi_bindgen_main()
}
