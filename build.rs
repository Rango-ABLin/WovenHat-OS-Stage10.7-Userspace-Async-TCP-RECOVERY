use std::env;
use std::path::PathBuf;

fn main() {
    let kernel = PathBuf::from(
        env::var_os("CARGO_BIN_FILE_KERNEL_wovenhat-kernel")
            .expect("WovenHat kernel artifact not found"),
    );

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR missing"));

    let uefi_path = out_dir.join("wovenhat-os-uefi.img");

    bootloader::UefiBoot::new(&kernel)
        .create_disk_image(&uefi_path)
        .expect("failed to create WovenHat OS UEFI image");

    println!(
        "cargo:rustc-env=WOVENHAT_UEFI_IMAGE={}",
        uefi_path.display()
    );
}
