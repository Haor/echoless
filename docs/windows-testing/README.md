# Windows Testing Docs

This directory is the network-fetchable entry point for the Windows-side agent.

## Read First

Start here:

1. `WINDOWS_AEC3_LOCALVQE_TEST_HANDOFF.md`
2. `WINDOWS_INSTALLED_APP_SMOKE_HANDOFF.md`
3. `WINDOWS_RTX_AEC_TEST_HANDOFF.md`

The correct order is:

1. Test Echoless AEC3 quality.
2. Test LocalVQE v1.3 standalone quality.
3. Prove the Windows installed Tauri app can find the sidecar CLI and bundled LocalVQE runtime/model.
4. Preserve diagnostic recordings.
5. Research or compare RTX / NVIDIA AFX AEC with those recordings.

Do not start from RTX AEC. RTX AEC is a later candidate/backend investigation, not the current product baseline.

## Research Files

The required research files are copied into:

- `../research/windows_aec_research.md`
- `../research/sonora_aec3_internal_map.md`
- `../research/reference_repos_exploration_report.md`
- `../research/cross_platform_architecture.md`

## Direct URLs

If the agent has GitHub access, it can fetch raw files from:

- <https://raw.githubusercontent.com/Haor/echoless/main/docs/windows-testing/WINDOWS_AEC3_LOCALVQE_TEST_HANDOFF.md>
- <https://raw.githubusercontent.com/Haor/echoless/main/docs/windows-testing/WINDOWS_INSTALLED_APP_SMOKE_HANDOFF.md>
- <https://raw.githubusercontent.com/Haor/echoless/main/docs/windows-testing/WINDOWS_RTX_AEC_TEST_HANDOFF.md>
- <https://raw.githubusercontent.com/Haor/echoless/main/docs/research/windows_aec_research.md>
- <https://raw.githubusercontent.com/Haor/echoless/main/docs/research/sonora_aec3_internal_map.md>
- <https://raw.githubusercontent.com/Haor/echoless/main/docs/research/reference_repos_exploration_report.md>
- <https://raw.githubusercontent.com/Haor/echoless/main/docs/research/cross_platform_architecture.md>

Because this repository is private, raw URLs may require authentication. The
more reliable path is:

```powershell
gh repo clone Haor/echoless
cd echoless
Get-Content .\docs\windows-testing\README.md
```

To download the current Windows artifact:

```powershell
gh run download 27058593366 --repo Haor/echoless --name echoless-windows-X64 --dir .\artifact
```
