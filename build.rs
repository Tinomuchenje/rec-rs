use std::{env, path::PathBuf};

fn main() {
    // Embed Info.plist into the Mach-O so TCC finds usage strings and a stable
    // bundle identifier even when the binary runs outside an .app bundle.
    //
    // Worktrees share one target dir, and cargo reuses this script's output
    // across them, so link a copy in OUT_DIR rather than a path into one
    // checkout (which breaks the link once that checkout is removed).
    let src = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("support/Info.plist");
    let plist = PathBuf::from(env::var("OUT_DIR").unwrap()).join("Info.plist");
    std::fs::copy(&src, &plist).unwrap();
    println!("cargo:rerun-if-changed=support/Info.plist");
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
