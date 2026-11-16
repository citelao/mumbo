// mumbo's tauri-build statically links the VC runtime by putting a dummy
// msvcrt.lib on the library search path. That search path leaks into
// dependents like this one, but the matching linker flags don't, so on Windows
// this binary links against nothing and fails. Link the CRT the same way.
fn main() {
    static_vcruntime::metabuild();
}
