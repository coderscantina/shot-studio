//! `shot theme use` writes what the core's switch writes, the same as the app and the MCP tool.

use std::process::Command;

use shot_core::{Brand, FileChange, Id, Themes};

#[test]
fn theme_use_writes_the_cores_switch() {
    let dir = tempfile::tempdir().unwrap();
    let brand = "theme dark\nsounds zoom=whoosh\n";
    let looks = [
        ("dark", "colors accent=\"#101418\"\n"),
        ("light", "camera handheld=40\n"),
    ];
    std::fs::create_dir_all(dir.path().join("brand/themes")).unwrap();
    std::fs::write(dir.path().join("brand/brand.kdl"), brand).unwrap();
    for (name, text) in looks {
        std::fs::write(dir.path().join(format!("brand/themes/{name}.kdl")), text).unwrap();
    }
    let before = Themes {
        brand: Brand::parse(brand).unwrap(),
        looks: looks
            .map(|(name, text)| (Id::new(name).unwrap(), Brand::parse_theme(text).unwrap()))
            .into(),
    };
    let after = before.activate(&Id::new("light").unwrap()).unwrap();
    let Ok([FileChange::Write { text, .. }]) = <[_; 1]>::try_from(after.changes(&before)) else {
        panic!("a switch writes one file");
    };

    let out = Command::new(env!("CARGO_BIN_EXE_shot"))
        .arg("--repo")
        .arg(dir.path())
        .args(["theme", "use", "light"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = std::fs::read_to_string(dir.path().join("brand/brand.kdl")).unwrap();
    assert_eq!(written, text);
    assert_eq!(written, "theme light\nsounds zoom=whoosh\n");
}
