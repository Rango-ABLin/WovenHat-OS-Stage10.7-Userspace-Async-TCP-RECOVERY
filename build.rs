use std::env;
use std::path::PathBuf;

fn main() {
    let kernel = PathBuf::from(
        env::var_os("CARGO_BIN_FILE_KERNEL_wovenhat-kernel")
            .expect("WovenHat kernel artifact not found"),
    );

    // Acceptance logs and other workspace files are not image inputs. Without
    // explicit inputs Cargo scans the whole package and can rebuild the image
    // on every stress boot. Track the actual artifact so kernel changes still
    // always regenerate the boot image.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", kernel.display());

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
