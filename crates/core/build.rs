fn main() {
    // Rust'a vcpkg üzerinden SDL3'ü bulmasını ve bağlamasını söylüyoruz
    // `cfg!()` would still compile this block on every platform, but vcpkg is
    // only a build-dependency on Windows, so gate it at compile time instead.
    #[cfg(target_os = "windows")]
    {
        unsafe { std::env::set_var("VCPKGRS_DYNAMIC", "1") };
        vcpkg::Config::new()
            .find_package("sdl3")
            .expect("SDL3 kütüphanesi vcpkg ile bulunamadı! VCPKG_ROOT ayarlı mı?");
    }
}