use std::fs;

use api_subway_analyzers::{AnalyzeOptions, Framework, analyze};

#[test]
fn malformed_sources_are_panic_free_and_deterministic() {
    let temporary = tempfile::tempdir().expect("temporary adversarial corpus");
    fs::write(
        temporary.path().join("package.json"),
        r#"{"dependencies":{"express":"5","next":"16"}}"#,
    )
    .expect("JavaScript manifest");
    fs::write(
        temporary.path().join("pyproject.toml"),
        "[project]\nname = \"adversarial\"\ndependencies = [\"fastapi\"]\n",
    )
    .expect("Python manifest");

    let mut random = DeterministicRandom::new(0x5eed_cafe_dead_beef);
    for index in 0..64 {
        let extension = if index % 2 == 0 { "ts" } else { "py" };
        let mut source = match extension {
            "ts" => "export async function GET(\n".to_owned(),
            _ => "@router.get(\"/fuzz\")\ndef handler(\n".to_owned(),
        };
        for _ in 0..1_024 {
            source.push(random.character());
        }
        fs::write(
            temporary
                .path()
                .join(format!("malformed-{index}.{extension}")),
            source,
        )
        .expect("adversarial source");
    }
    fs::write(temporary.path().join("invalid-utf8.ts"), [0xff, 0xfe, 0xfd])
        .expect("invalid UTF-8 source");

    let mut options = AnalyzeOptions::new(temporary.path());
    options.frameworks = vec![Framework::Next, Framework::Express, Framework::FastApi];
    let first = analyze(&options).expect("first adversarial analysis");
    let second = analyze(&options).expect("second adversarial analysis");

    assert_eq!(first, second);
    assert!(first.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "source-read"
            && diagnostic
                .message
                .contains("stream did not contain valid UTF-8")
    }));
}

struct DeterministicRandom(u64);

impl DeterministicRandom {
    const ALPHABET: [char; 30] = [
        '\0', '\n', '\r', '\t', ' ', '(', ')', '[', ']', '{', '}', '<', '>', '/', '\\', '\'', '"',
        '`', ':', ';', ',', '.', '=', '$', '_', 'a', '9', 'é', 'λ', '🛤',
    ];

    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn character(&mut self) -> char {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        let index = usize::try_from(self.0 % Self::ALPHABET.len() as u64)
            .expect("alphabet index fits usize");
        Self::ALPHABET[index]
    }
}
