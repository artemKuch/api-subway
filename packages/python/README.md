# api-subway for Python

The platform wheel contains the native `api-subway` binary. The wrapper never downloads code at runtime.

Published wheels target macOS 11+ (arm64/x64), manylinux 2.35/glibc (arm64/x64), and Windows x64. musl/Alpine is not supported in v0.1.

```bash
uvx api-subway generate . --out docs/api-subway
```
