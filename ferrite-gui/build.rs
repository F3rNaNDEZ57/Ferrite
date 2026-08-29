//! Embeds `assets/ferrite.ico` into the compiled `.exe`, so Explorer, the
//! taskbar, and Alt-Tab show Ferrite's own icon instead of a generic one -
//! independent of the *running* window's icon, which `main.rs` sets
//! separately via `egui::IconData`. v1 is Windows-only (see the vault's
//! `v1-scope.md`), so no non-Windows fallback is needed here.

fn main() {
    winresource::WindowsResource::new()
        .set_icon("assets/ferrite.ico")
        .compile()
        .expect("embedding assets/ferrite.ico into the exe");
}
