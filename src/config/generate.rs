//! First-run default generation: the config.yaml template, secure file/dir
//! permissions (600/700) and the random `embedded_secret`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::info;
use rand::Rng;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use super::codec_registry::{resolve_watchers, CodecRegistry};
use super::global::{self, GlobalConfig};
use super::load::create_directories;
use super::presets::generate_preset_files;
use super::validate::validate_config;
use super::watch::{self, WatchConfig, WatchType};

/// Secure file permissions: 600 (owner read/write only)
#[cfg(unix)]
const SECURE_FILE_MODE: u32 = 0o600;

/// Secure directory permissions: 700 (owner read/write/execute only)
#[cfg(unix)]
const SECURE_DIR_MODE: u32 = 0o700;
/// Generate a cryptographically random embedded_secret (64 hex chars).
fn generate_secret() -> String {
    let mut rng = rand::rngs::OsRng;
    (0..32)
        .map(|_| format!("{:02x}", rng.gen::<u8>()))
        .collect()
}

#[cfg(unix)]
pub(crate) fn write_secure_file(path: &Path, content: &str) -> Result<()> {
    use std::fs::OpenOptions;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(SECURE_FILE_MODE)
        .open(path)
        .with_context(|| format!("Cannot create file {}", path.display()))?;
    std::io::Write::write_all(&mut file, content.as_bytes())
        .with_context(|| format!("Cannot write to {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn write_secure_file(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| format!("Cannot write to {}", path.display()))
}

#[cfg(unix)]
pub(crate) fn set_secure_dir_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(SECURE_DIR_MODE);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("Cannot set permissions on {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_secure_dir_mode(_path: &Path) -> Result<()> {
    Ok(())
}

/// Generate default config files on first run.
pub(crate) fn generate_default_config(
    config_path: &Path,
) -> Result<(GlobalConfig, Vec<WatchConfig>, CodecRegistry)> {
    let config_dir = config_path.parent().unwrap_or(Path::new("config"));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let abs = cwd.canonicalize().unwrap_or(cwd);

    let inputs_base = abs.join("inputs");
    let outputs_base = abs.join("outputs");
    let logs_base = abs.join("logs");

    let mut default_global = GlobalConfig {
        log: global::LogConfig {
            errors_file: logs_base.join("errors.log").to_string_lossy().to_string(),
            app_log_file: logs_base.join("app.log").to_string_lossy().to_string(),
            ..Default::default()
        },
        healthcheck: global::HealthcheckConfig::default(),
        disk_space: global::DiskSpaceConfig::default(),
        history: global::HistoryConfig {
            file: logs_base.join("history.json").to_string_lossy().to_string(),
            ..Default::default()
        },
        ..Default::default()
    };

    // Use the same secret in the in-memory config and the generated YAML.
    let secret = generate_secret();
    default_global.embedded_secret = secret.clone();

    let default_watchers = vec![WatchConfig {
        name: "default".to_string(),
        watch_folder: inputs_base.join("default").to_string_lossy().to_string() + "/",
        output_folder: outputs_base.join("default").to_string_lossy().to_string() + "/",
        subfolders: vec![
            watch::Subfolder {
                name: "gpu".to_string(),
                description: Some("GPU-accelerated encoding".to_string()),
            },
            watch::Subfolder {
                name: "archive".to_string(),
                description: Some("High-quality archival".to_string()),
            },
        ],
        watch_type: WatchType::Video {
            rules: vec![
                watch::VideoRule {
                    preset: "libx264".to_string(),
                    subfolder: None,
                    input_extensions: vec![
                        ".mp4".into(),
                        ".avi".into(),
                        ".mkv".into(),
                        ".mov".into(),
                        ".webm".into(),
                        ".flv".into(),
                        ".wmv".into(),
                        ".mpeg".into(),
                        ".mpg".into(),
                        ".ts".into(),
                        ".mts".into(),
                        ".mxf".into(),
                    ],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: Some(true),
                    min_duration_ratio: Some(0.9),
                },
                watch::VideoRule {
                    preset: "h264_nvenc".to_string(),
                    subfolder: Some("gpu".to_string()),
                    input_extensions: vec![".mxf".into(), ".mts".into(), ".mov".into()],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: Some(true),
                    min_duration_ratio: Some(0.9),
                },
                watch::VideoRule {
                    preset: "libx265_high".to_string(),
                    subfolder: Some("archive".to_string()),
                    input_extensions: vec![
                        ".mxf".into(),
                        ".mts".into(),
                        ".mov".into(),
                        ".mkv".into(),
                    ],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: Some(true),
                    min_duration_ratio: Some(0.95),
                },
            ],
        },
    }];

    let inputs_str = inputs_base.display();
    let outputs_str = outputs_base.display();
    let logs_str = logs_base.display();

    let default_yaml = format!(
        r#"# ─────────────────────────────────────────────────────────────
# ConvWatcher — Configuração Padrão
# Gerada automaticamente na primeira execução.
# ─────────────────────────────────────────────────────────────

global:
  # Intervalo de varredura das pastas monitoradas.
  # O sistema verifica periodicamente se há arquivos novos ou modificados.
  # Formato: <numero> + sufixo (s=segundos, ms=milissegundos, m=minutos)
  # Default: 2s
  file_check_interval: 2s

  # Tempo de estabilidade: quanto tempo após o arquivo parar de
  # crescer antes de iniciar a conversão.
  # Previne processar arquivos que ainda estão sendo copiados/uploadados.
  # Formato: <numero> + sufixo (s, ms)
  # Default: 5s
  stable_time: 5s

  # Caminho absoluto do binário FFmpeg.
  # Usado para conversão de vídeo e áudio.
  # Obrigatório se o watcher usar tipo video ou audio.
  # Default: /usr/bin/ffmpeg
  ffmpeg_path: /usr/bin/ffmpeg

  # Caminho absoluto do binário FFprobe.
  # Opcional — se omitido, usa o mesmo diretório do ffmpeg_path.
  # ffprobe_path: /usr/bin/ffprobe

  # Máximo de conversões executando simultaneamente.
  # Aumentar = mais CPU/RAM. Diminuir = menos contenção de recursos.
  # Default: 4
  max_concurrent: 4

  # Intervalo de hot-reload: com que frequência o sistema re-scaneia
  # os arquivos de configuração em busca de mudanças.
  # 0 = desabilita hot-reload.
  # Formato: <numero> + sufixo (s, m)
  # Default: 5m
  refresh_interval: 5m

  # Secret para validação de configs embutidas (embedded overrides) e agents remotos.
  # GERADO AUTOMATICAMENTE na primeira execução. Guarde-o com segurança.
  # Para regenerar: delete a linha e reinicie (ou edite manualmente).
  embedded_secret: {secret}

  # Intervalo de scan para configs embutidas nas pastas monitoradas.
  # 0 = desabilita scan de embedded configs.
  # Default: 10s
  embedded_scan_interval: 10

  # Paths para arquivos de codec presets, relativos à pasta config/.
  codec_presets:
    video: video_codecs.yaml   # Presets de codecs de vídeo
    audio: audio_codecs.yaml   # Presets de codecs de áudio
    image: image_codecs.yaml   # Presets de formatos de imagem
    pdf: pdf_presets.yaml      # Presets de processamento PDF
    document: document_presets.yaml  # Presets de conversão de documentos

  log:
    # Caminho absoluto do arquivo de log de erros.
    # O diretório é criado automaticamente se não existir.
    # Default: {logs}/errors.log
    errors_file: {logs}/errors.log

    # Caminho absoluto do arquivo de log da aplicação (conversions, info).
    # O diretório é criado automaticamente se não existir.
    # Default: {logs}/app.log
    app_log_file: {logs}/app.log

    # Redact secrets from error logs to prevent credential leaks.
    # When true, replaces embedded_secret values and patterns like password=, token= with ***REDACTED***.
    # Default: true
    redact_secrets: true

    # Número máximo de arquivos de log rotacionados a manter.
    # Default: 30
    max_log_files: 30

    # Tamanho máximo de cada arquivo de log antes da rotação (MB).
    # Default: 100
    max_log_size_mb: 100

    # Tamanho máximo do arquivo de log de erros (MB).
    # Default: 50
    max_error_log_size_mb: 50

  healthcheck:
    # Porta HTTP do painel de saúde.
    # Acesse em http://<bind_address>:<http_port>/dashboard
    # Default: 8080
    http_port: 8080

    # Endereço de bind do servidor HTTP.
    # Use "127.0.0.1" para acesso local apenas; "0.0.0.0" expõe o painel.
    # Default: "127.0.0.1"
    bind_address: 127.0.0.1

    # Senha do dashboard. Enviada internamente como Authorization: Bearer <senha>
    # nas chamadas de API/logs. /health permanece aberto para o HEALTHCHECK do
    # container. Vazio/ausente = sem autenticação.
    # Aceita a chave antiga auth_token (compatibilidade 0.9.9).
    # Default: "" (desabilitado)
    auth_password: ""

  disk_space:
    # Intervalo entre verificações de espaço em disco.
    # Formato: <numero> + sufixo (s, m)
    # Default: 60s
    check_interval: 60s

    # Limiar de espaço livre. Quando o espaço livre cai abaixo
    # deste valor, as conversões são pausadas até espaço ser liberado.
    # Pode ser: <numero> (MB), <numero>Gb, ou <numero>% (percentual do total)
    # Exemplos: 500 (MB), 5Gb, 10%
    # Default: 500 (MB)
    threshold: 500

    # Verificar espaço no disco da pasta de saída.
    # Default: false
    check_output: false

    # Verificar espaço no disco da pasta de entrada.
    # Default: false
    check_watch: false

  history:
    # Persistir histórico de conversões em disco.
    # Quando false, o histórico é perdido ao reiniciar.
    # Default: false
    persistent: false

    # Caminho do arquivo de histórico (usado apenas se persistent: true).
    # Default: {logs}/history.json
    file: {logs}/history.json

    # Número máximo de registros no histórico.
    # Default: 500
    max_records: 500

watchers:
  # Cada watcher monitora uma pasta e converte arquivos de um tipo.
  # É possível ter múltiplos watchers com tipos diferentes.

  - name: default
    # Caminho absoluto da pasta a monitorar.
    # Obrigatório. Deve ser um path absoluto.
    watch_folder: {inputs}/default/

    # Caminho absoluto onde os arquivos convertidos serão salvos.
    # Obrigatório. Deve ser um path absoluto.
    output_folder: {outputs}/default/

    # Tipo de conversão: video | image | audio | pdf | document | custom
    # Determina qual processador manipula os arquivos.
    # Obrigatório.
    type: video

    # Subpastas declaradas para roteamento de regras.
    # Cria diretórios ->{{name}}/ dentro da watch_folder.
    # Arquivos colocados em ->{{name}}/ são roteados a regras com subfolder: <name>.
    subfolders:
      - name: gpu
        description: "GPU-accelerated encoding"
      - name: archive
        description: "High-quality archival"

    # Regras de conversão — arquivos são verificados em ordem.
    # A primeira regra que match é usada.
    rules:
      # Regra raiz: match arquivos colocados diretamente na watch_folder.
      - input_extensions: [.mp4, .avi, .mkv, .mov, .webm, .flv, .wmv, .mpeg, .mpg, .ts, .mts, .mxf]
        # Nome do preset (obrigatório). Definido em video_codecs.yaml.
        # O preset define codec, qualidade, áudio, e extensão de saída.
        preset: libx264
        # Template do nome do arquivo de saída.
        # Placeholders: {{base}}, {{codec}}, {{ext}}, {{num}}
        # Default: "{{base}}_{{codec}}_{{num}}.{{ext}}"
        output_name: "{{base}}_{{codec}}_{{num}}.{{ext}}"
        # Verificar duração do output vs input.
        # Previne saída corrompida / truncada.
        # Default: true
        check_duration: true
        # Razão mínima aceitável de duração (output/input).
        # 0.9 = output deve ter pelo menos 90% da duração do input.
        # Default: 0.9
        min_duration_ratio: 0.9

      # Regra de subpasta: match arquivos em ->gpu/
      - subfolder: gpu
        input_extensions: [.mxf, .mts, .mov]
        preset: h264_nvenc
        output_name: "{{base}}_gpu.{{ext}}"
        check_duration: true
        min_duration_ratio: 0.9

      # Regra de subpasta: match arquivos em ->archive/
      - subfolder: archive
        input_extensions: [.mxf, .mts, .mov, .mkv]
        preset: libx265_high
        output_name: "{{base}}_archive.{{ext}}"
        check_duration: true
        min_duration_ratio: 0.95
"#,
        inputs = inputs_str,
        outputs = outputs_str,
        logs = logs_str,
        secret = secret,
    );

    write_secure_file(config_path, &default_yaml)
        .with_context(|| format!("Cannot write {}", config_path.display()))?;
    info!("Created default config: {}", config_path.display());

    // Set secure permissions on config directory
    set_secure_dir_mode(config_dir)?;

    // Generate codec preset files
    generate_preset_files(config_dir)?;

    // Load the registry from generated files
    let registry = CodecRegistry::load(config_dir, &default_global.codec_presets)?;

    // Resolve watchers
    let resolved = resolve_watchers(default_watchers, &registry)?;

    // Validate and create directories
    validate_config(&default_global, &resolved)?;
    create_directories(&default_global, &resolved, config_dir)?;

    Ok((default_global, resolved, registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generated_default_config_is_reparseable() {
        // Regression: the generated config.yaml template must parse back on the
        // next startup. A broken template (wrong indentation for `global` /
        // `log` / watcher fields) used to kill the process on run #2.
        let tmp = std::env::temp_dir().join(format!("cw-gencfg-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let config_path = tmp.join("config.yaml");

        let (global, watchers, _registry) =
            generate_default_config(&config_path).expect("template generation must succeed");
        assert_eq!(watchers.len(), 1);
        assert!(
            !global.embedded_secret.is_empty(),
            "first-run in-memory config must carry the generated secret"
        );
        assert_eq!(global.embedded_scan_interval_s, 10);

        // Round-trip: the YAML written by generate_default_config must parse.
        let content = std::fs::read_to_string(&config_path).unwrap();

        #[derive(serde::Deserialize)]
        struct ConfigFile {
            global: Option<GlobalConfig>,
            watchers: Vec<WatchConfig>,
        }
        let parsed: ConfigFile = serde_yaml::from_str(&content).unwrap_or_else(|e| {
            panic!("generated config must parse back (Bug A regression): {}", e)
        });
        let g = parsed.global.unwrap_or_default();
        assert!(
            g.log.errors_file.starts_with('/'),
            "log.errors_file must be absolute, got {}",
            g.log.errors_file
        );
        assert_eq!(parsed.watchers.len(), 1);
        assert_eq!(
            g.embedded_secret, global.embedded_secret,
            "generated YAML and in-memory config must use the same secret"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
