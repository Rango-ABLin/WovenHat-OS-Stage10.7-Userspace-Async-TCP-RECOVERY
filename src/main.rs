fn main() {
    if std::env::args().nth(1).as_deref() == Some("--print-image") {
        println!("{}", env!("WOVENHAT_UEFI_IMAGE"));
    } else {
        println!("WovenHat OS build host");
    }
}
