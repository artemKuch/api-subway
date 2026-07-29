# api-subway for npm

This package selects the native `api-subway` binary for the current platform. It does not download binaries at runtime and does not execute the analyzed application.

Supported binaries are macOS 11+ (arm64/x64), glibc-based Linux with glibc 2.35+ (arm64/x64), and Windows x64. musl/Alpine is not supported in v0.1.

```bash
npx api-subway generate . --out docs/api-subway
```
