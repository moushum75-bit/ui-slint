// Compiles ui/app.slint into Rust code at build time.
// main.rs pulls it in with slint::include_modules!().
fn main() {
    slint_build::compile("ui/app.slint").expect("Slint build failed");
}
