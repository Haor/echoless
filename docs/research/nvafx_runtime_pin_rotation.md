# NvAFX Runtime Pin Rotation

本文档说明 Echoless RTX AEC runtime 资产的 SHA256 pin 如何维护。目标是让
`nvafx download-install` 的完整性校验有单一事实源,并避免发新版 runtime 时漏改 hash。

## 当前信任模型

- 默认 release tag: `rtx-aec-runtime-win64-2.1.0-aec48-preview.1`
- 默认 release 的信任锚在代码中:
  `crates/echoless-cli/src/nvafx_install.rs` 的 `NVAFX_DEFAULT_RELEASE_PINS`
- 对默认 tag,内置 pin 优先。下载到的 `SHA256SUMS.txt` 只作为交叉校验:
  如果 release sums 和内置 pin 不一致,安装必须失败。
- 对非默认 tag,CLI 可以使用 release 提供的 `SHA256SUMS.txt`。如果没有 sums,
  CLI 会重新下载并只记录实际 hash;此时应显式传 `--common-sha256` / `--model-sha256`
  或把该 tag 提升为新的默认 pin。

## 当前 pin 表

| Asset | SHA256 |
| --- | --- |
| `echoless-rtx-aec-common-runtime-win64-2.1.0.zip` | `dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb` |
| `echoless-rtx-aec-model-win64-2.1.0-turing-aec48.zip` | `951e03bb144156f4b27cbf2caa6930f9dabc3f1cb26a0afd9d9523f4d286dae9` |
| `echoless-rtx-aec-model-win64-2.1.0-ampere-aec48.zip` | `066e06ec18a7d4509675411a1e050e11b0cfc4fee30d69d783871333018c9ab9` |
| `echoless-rtx-aec-model-win64-2.1.0-ada-aec48.zip` | `92170e6a259f9093397b93cf4385759c36697ecb9e308322405bce1abcb8e3df` |
| `echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip` | `0e75bb7442d317990ef0d5a6477105f86b9bbae1c2c5e4a6bdfb8d4e9f42df5b` |

## 轮换步骤

1. 生成新的 common zip 和各 GPU 架构 model zip。
2. 在干净机器或 CI artifact 上计算 SHA256:

   ```bash
   shasum -a 256 echoless-rtx-aec-*.zip
   ```

   Windows 可用:

   ```powershell
   Get-FileHash .\echoless-rtx-aec-*.zip -Algorithm SHA256
   ```

3. 更新 `crates/echoless-cli/src/nvafx_install.rs`:
   - `DEFAULT_NVAFX_RELEASE_TAG`
   - `NVAFX_COMMON_RUNTIME_ASSET`(如果 common asset 名变化)
   - `NVAFX_DEFAULT_RELEASE_PINS`
4. 生成并上传同一组 asset 的 `SHA256SUMS.txt`。它必须和内置 pin 完全一致。
5. 更新面向前端/测试的 runtime 文档:
   - `docs/research/rtx_aec_runtime_distribution.md`
   - `docs/frontend/RTX_DOWNLOAD_INSTALL_HANDOFF.md`
   - `docs/windows-testing/WINDOWS_RTX_AEC_TEST_HANDOFF.md`(如测试分支需要)
6. 跑本地验证:

   ```bash
   cargo test -p echoless-cli nvafx_install --locked
   cargo fmt -p echoless-cli --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   ```

7. 在 Windows RTX 机器上验证:

   ```powershell
   .\echoless.exe nvafx download-install --json
   .\echoless.exe nvafx doctor --json
   ```

   验证重点:
   - `download-install` 能选择正确 GPU 架构 asset。
   - 缓存命中时仍会校验 SHA256。
   - 故意改坏 `SHA256SUMS.txt` 或本地 zip 时会失败。

## 修改原则

- 不要让默认 tag 依赖远端 `SHA256SUMS.txt` 作为唯一信任源。
- 不要只更新文档表格而不更新 `NVAFX_DEFAULT_RELEASE_PINS`。
- 不要只更新一部分架构;Turing / Ampere / Ada / Blackwell 四个 model asset 要一起确认。
- 如果换 SDK/runtime 版本,同时核对 `echoless_processors::nvafx::SDK_VERSION` 和
  `RUNTIME_FILE_VERSION` 的 doctor 输出是否仍正确。
