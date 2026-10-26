//! Affiche le matériel détecté et le profil de machine : `cargo run -p claudio-hw --example detect`.
fn main() {
    let hw = claudio_hw::HardwareInfo::detect();
    println!("{hw:#?}");
    println!("{:#?}", hw.profile());
}
