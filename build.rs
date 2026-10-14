// En Windows, el icono de Forge va dentro de forge.exe: el explorador, el menú Inicio, el
// buscador y los accesos directos lo toman del archivo (el de la ventana se pone al abrir).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    // Desde macOS (comprobaciones con zig) no hay compilador de recursos: se avisa y sigue.
    if let Err(e) = res.compile() {
        println!("cargo:warning=forge.exe sin icono: {e}");
    }
}
