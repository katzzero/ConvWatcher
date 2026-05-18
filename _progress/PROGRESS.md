# ConvWatcher — Progresso da Implementação

**Data de início:** 2026-05-18  
**Status global:** 🟢 Concluído  
**Última alteração:** Refatoração single-type + watchs/ com secret + inputs/outputs padrão

---

## Fase 1: Fundação ✅
- [x] Estrutura de diretórios
- [x] Cargo.toml
- [x] .gitignore

## Fase 2: Config ✅
- [x] src/config/mod.rs (refatorado — merge manifesto + watchs/)
- [x] src/config/global.rs (refatorado — + inputs_dir, outputs_dir)
- [x] src/config/watch.rs (refatorado — WatchType enum, single-type)
- [x] src/config/embedded.rs (refatorado — + secret, WatchType)

## Fase 3: Utilitários + CLI ✅
- [x] src/cli.rs
- [x] src/utils/mod.rs
- [x] src/utils/hardware.rs
- [x] src/utils/path.rs
- [x] src/logs/mod.rs
- [x] src/logs/error_logger.rs

## Fase 4: Processadores ✅
- [x] src/processor/mod.rs
- [x] src/processor/job.rs (refatorado — struct em vez de enum)
- [x] src/processor/video.rs
- [x] src/processor/image.rs
- [x] src/processor/audio.rs
- [x] src/processor/pdf.rs
- [x] src/processor/document.rs
- [x] src/processor/external.rs
- [x] src/processor/disk.rs
- [x] src/processor/namer.rs

## Fase 5: Watcher ✅
- [x] src/watcher/mod.rs
- [x] src/watcher/monitor.rs (refatorado — single-type + validate_and_promote_config)
- [x] src/watcher/embedded.rs (refatorado — scan config/watchs/ com merge + secret)

## Fase 6: Health Server ✅
- [x] src/health/mod.rs
- [x] src/health/server.rs (refatorado — WatcherInfo single-type)
- [x] src/health/dashboard.html

## Fase 7: Main ✅
- [x] src/main.rs (refatorado — dispatcher single-type)

## Fase 8: Config Files + Exemplos ✅
- [x] config/global.yaml (refatorado — + inputs_dir, outputs_dir)
- [x] config/watchers.yaml (refatorado — manifesto single-type 6 watchers)
- [x] examples/watcher_sample.yaml (refatorado — 7 exemplos)

## Fase 9: Docker + CI/CD + Scripts ✅
- [x] Dockerfile
- [x] docker-compose.yml
- [x] docker-bake.json
- [x] .dockerignore
- [x] .github/workflows/docker.yml
- [x] scripts/install_linux.sh
- [x] scripts/install_macos.sh
- [x] scripts/install_windows.ps1
- [x] scripts/build-arm64.sh
- [x] scripts/build-docker-arm64.sh

---

## Regras do sistema

| Regra | Descrição |
|-------|-----------|
| Single-type | Cada watcher é de UM tipo (video, image, audio, pdf, document, custom) |
| watch_folder | `./inputs/<name>/` (default, override via global.inputs_dir + manifesto) |
| output_folder | `./outputs/<name>-output/` (default, override via manifesto ou watchs/) |
| secret | watchs/<name>.yaml deve ter `secret` == `global.embedded_secret` |
| Tipo travado | Override deve ter o mesmo tipo que o manifesto |
| Promoção | `.yaml` dropado na watch → validado → promovido p/ config/watchs/ |
| .invalid | Config inválida → cria `<name>.invalid` vazio. Usuário deleta p/ retry |
| .yaml.old | Config promovida → original renomeado p/ `.yaml.old` |

## Hierarquia de output_folder

```
1. watchs/<name>.yaml  →  output_folder (override, opcional)
2. watchers.yaml       →  output_folder (manifesto, opcional)
3. global.yaml         →  ./outputs/<name>-output/ (default)
```

## Compilação

✅ `cargo build` bem-sucedido (4 warnings não-críticos)
