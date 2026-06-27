# 维护指南

这份文档记录本项目日常维护时推荐执行的检查流程，重点避免 Cargo feature 组合误用。

## Feature 组合约定

`official-api` 与 `no-official-api` 是互斥 feature，不能使用 `--all-features`。

推荐维护时覆盖以下组合：

1. 默认组合：`official-api + tts + clipboard + clipboard-arboard`
2. 无官方 API 组合：`--no-default-features --features no-official-api`
3. 跨平台/轻量构建可按需测试：`--no-default-features --features official-api,tts-native,clipboard`

## 本地检查

Windows PowerShell：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\maintain.ps1
```

Linux/macOS/Git Bash：

```bash
bash ./scripts/maintain.sh
```

脚本会执行：

- `cargo fmt --all -- --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --no-default-features --features no-official-api`
- `cargo clippy --no-default-features --features no-official-api --all-targets -- -D warnings`
- `cargo tree -d`

如果只想快速验证默认路径：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\maintain.ps1 -SkipNoOfficial -SkipTree
```

## 提交前建议

- 业务行为变更：补单元测试或集成测试。
- 依赖变更：查看 `cargo tree -d`，避免引入明显重复/过重依赖。
- feature 变更：同步更新本文件、`Cargo.toml` feature 注释和 CI。
- 发布相关变更：确认 `.github/workflows/build-rust.yml` 的目标平台仍能覆盖。

## 已知维护重点

- PDF 相关依赖链会引入旧版 `image/time`，如果包体积成为问题，可考虑把 PDF 输出拆成可选 feature。
- 默认 `tts` 依赖较重；如需更轻发布包，可考虑提供默认关闭 TTS 的 lite 构建。
- Web UI 与 TUI 都是用户入口，涉及配置/下载流程时建议同时验证两边行为。
