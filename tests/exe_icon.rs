#![cfg(windows)]
// forge.exe lleva el icono de Forge dentro: el explorador, el menú Inicio, el buscador y
// el acceso directo lo toman del archivo, no de la ventana.

#[test]
fn forge_exe_has_an_embedded_icon() {
    use windows_sys::Win32::UI::Shell::ExtractIconExW;
    let path: Vec<u16> = env!("CARGO_BIN_EXE_forge")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: ruta terminada en 0; con índice -1 solo cuenta los iconos, sin crearlos.
    let icons = unsafe {
        ExtractIconExW(
            path.as_ptr(),
            -1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    assert!(icons > 0, "forge.exe no lleva icono");
}
