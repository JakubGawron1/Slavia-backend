fn main() {
    // libsql/hrana na Windows: pierwsze zapytania do Turso potrafią przepełnić domyślny stos (~1 MB).
    #[cfg(all(windows, target_env = "msvc"))]
    println!("cargo:rustc-link-arg=/STACK:33554432");
}
