from pathlib import Path

manifest = Path("Cargo.toml")
text = manifest.read_text(encoding="utf-8")
text = text.replace('official-api = ["dep:tomato-novel-official-api"]', 'official-api = []')
text = "\n".join(
    line for line in text.splitlines() if not line.startswith("tomato-novel-official-api =")
)
manifest.write_text(text + "\n", encoding="utf-8")
