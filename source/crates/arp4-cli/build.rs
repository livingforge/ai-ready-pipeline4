use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for agent in ["claude", "github"] {
        for skill in ["arp4", "arp4-setup"] {
            let dir = root.join("surface/skills").join(skill);
            let read = |name: &str| {
                let path = dir.join(name);
                println!("cargo:rerun-if-changed={}", path.display());
                fs::read_to_string(path)
                    .unwrap()
                    .replace("\r\n", "\n")
                    .trim()
                    .to_owned()
            };
            let fragment = |text: String| {
                text.lines()
                    .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let common = fragment(read("frontmatter.common.yaml"));
            let platform = fragment(read(&format!("frontmatter.{agent}.yaml")));
            let body = read("body.md");
            let front = [common, platform]
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(
                out.join(format!("{agent}-{skill}.md")),
                format!("---\n{front}\n---\n\n{body}\n"),
            )
            .unwrap();
        }
    }
}
