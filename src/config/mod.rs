pub mod codec_registry;
pub mod global;
pub mod watch;
pub mod embedded;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::{info, warn};

use codec_registry::{CodecRegistry, resolve_watchers};
use global::GlobalConfig;
use watch::{WatchConfig, WatchType};

const CONFIG_PATH: &str = "config/config.yaml";

/// Load all configuration from a single config.yaml file.
/// Returns (GlobalConfig, Vec<WatchConfig>, CodecRegistry).
pub fn load_config(custom_path: Option<&Path>) -> Result<(GlobalConfig, Vec<WatchConfig>, CodecRegistry)> {
    let path = custom_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(CONFIG_PATH));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create directory {}", parent.display()))?;
    }

    // Generate default config if it doesn't exist
    if !path.exists() {
        let default = generate_default_config(&path)?;
        let global = default.0.clone();
        let registry = default.2.clone();
        let watchers = default.1.clone();
        return Ok((global, watchers, registry));
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;

    #[derive(serde::Deserialize)]
    struct ConfigFile {
        global: Option<GlobalConfig>,
        watchers: Vec<WatchConfig>,
    }

    let config_file: ConfigFile = match serde_yaml::from_str(&content) {
        Ok(cf) => cf,
        Err(e) => {
            warn!("Failed to parse {}: {}", path.display(), e);
            let default = generate_default_config(&path)?;
            return Ok(default);
        }
    };

    let global = config_file.global.unwrap_or_default();

    // Determine config directory for loading presets
    let config_dir = path.parent().unwrap_or(Path::new("config")).to_path_buf();

    // Check if any preset file is missing — if so, regenerate all presets
    let presets_ok = [
        &config_dir.join(&global.codec_presets.video),
        &config_dir.join(&global.codec_presets.audio),
        &config_dir.join(&global.codec_presets.image),
        &config_dir.join(&global.codec_presets.pdf),
        &config_dir.join(&global.codec_presets.document),
        &config_dir.join(&global.codec_presets.custom),
    ].iter().all(|p| p.exists());

    if !presets_ok {
        warn!("One or more preset files missing — regenerating defaults");
        generate_preset_files(&config_dir)?;
    }

    // Load codec presets
    let registry = CodecRegistry::load(&config_dir, &global.codec_presets)
        .with_context(|| "Failed to load codec presets")?;

    // Resolve watchers (merge presets into rules)
    let watchers = resolve_watchers(config_file.watchers, &registry)
        .with_context(|| "Failed to resolve watcher presets")?;

    // Validate paths
    validate_config(&global, &watchers)?;

    // Create missing directories
    create_directories(&global, &watchers)?;

    Ok((global, watchers, registry))
}

fn validate_config(global: &GlobalConfig, watchers: &[WatchConfig]) -> Result<()> {
    // Validate global paths
    validate_absolute_path(&global.log.errors_file, "log.errors_file")?;
    validate_absolute_path(&global.history.file, "history.file")?;

    // Validate watcher paths
    for watcher in watchers {
        if watcher.watch_folder.is_empty() {
            anyhow::bail!(
                "Watcher '{}': watch_folder must be an absolute path, got empty string",
                watcher.name
            );
        }
        validate_absolute_path(&watcher.watch_folder, &format!("watcher '{}'.watch_folder", watcher.name))?;

        if watcher.output_folder.is_empty() {
            anyhow::bail!(
                "Watcher '{}': output_folder must be an absolute path, got empty string",
                watcher.name
            );
        }
        validate_absolute_path(&watcher.output_folder, &format!("watcher '{}'.output_folder", watcher.name))?;

        // Validate rules have non-empty input_extensions
        match &watcher.watch_type {
            WatchType::Video { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', video rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Image { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', image rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Audio { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', audio rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Pdf { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', pdf rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Document { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', document rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Custom { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', custom rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

fn validate_absolute_path(path: &str, label: &str) -> Result<()> {
    let p = Path::new(path);
    if !p.is_absolute() {
        anyhow::bail!(
            "{}: must be an absolute path, got '{}'. Example: /app/inputs/default/",
            label, path
        );
    }
    Ok(())
}

fn create_directories(global: &GlobalConfig, watchers: &[WatchConfig]) -> Result<()> {
    // Create log directory
    if let Some(parent) = Path::new(&global.log.errors_file).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create log directory {}", parent.display()))?;
    }

    // Create history file directory
    if let Some(parent) = Path::new(&global.history.file).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create history directory {}", parent.display()))?;
    }

    // Create watcher directories
    for watcher in watchers {
        fs::create_dir_all(&watcher.watch_folder)
            .with_context(|| format!("Cannot create watch folder {}", watcher.watch_folder))?;
        fs::create_dir_all(&watcher.output_folder)
            .with_context(|| format!("Cannot create output folder {}", watcher.output_folder))?;
    }

    // Create embedded configs directory
    fs::create_dir_all("config/watchs")
        .with_context(|| "Cannot create config/watchs directory")?;

    Ok(())
}

/// Generate default config files on first run.
fn generate_default_config(config_path: &Path) -> Result<(GlobalConfig, Vec<WatchConfig>, CodecRegistry)> {
    let config_dir = config_path.parent().unwrap_or(Path::new("config"));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let abs = cwd.canonicalize().unwrap_or(cwd);

    let inputs_base = abs.join("inputs");
    let outputs_base = abs.join("outputs");
    let logs_base = abs.join("logs");

    let default_global = GlobalConfig {
        log: global::LogConfig {
            errors_file: logs_base.join("errors.log").to_string_lossy().to_string(),
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
                        ".mp4".into(), ".avi".into(), ".mkv".into(), ".mov".into(),
                        ".webm".into(), ".flv".into(), ".wmv".into(), ".mpeg".into(),
                        ".mpg".into(), ".ts".into(), ".mts".into(), ".mxf".into(),
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
                    input_extensions: vec![".mxf".into(), ".mts".into(), ".mov".into(), ".mkv".into()],
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

    let default_yaml = format!(r#"# ─────────────────────────────────────────────────────────────
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

  # Secret para validação de configs embutidas (embedded overrides).
  # Se vazio, embedded configs são aceitas sem validação de secret.
  # Default: ""
  embedded_secret: ""

  # Intervalo de scan para configs embutidas nas pastas monitoradas.
  # 0 = desabilita scan de embedded configs.
  # Default: 0
  embedded_scan_interval: 0

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
    # Use "127.0.0.1" para acesso local apenas.
    # Default: "0.0.0.0"
    bind_address: 0.0.0.0

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
    );

    fs::write(config_path, &default_yaml)
        .with_context(|| format!("Cannot write {}", config_path.display()))?;
    info!("Created default config: {}", config_path.display());

    // Generate codec preset files
    generate_preset_files(config_dir)?;

    // Load the registry from generated files
    let registry = CodecRegistry::load(config_dir, &default_global.codec_presets)?;

    // Resolve watchers
    let resolved = resolve_watchers(default_watchers, &registry)?;

    // Validate and create directories
    validate_config(&default_global, &resolved)?;
    create_directories(&default_global, &resolved)?;

    Ok((default_global, resolved, registry))
}

fn generate_preset_files(config_dir: &Path) -> Result<()> {
    // Generate video_codecs.yaml
    let video_path = config_dir.join("video_codecs.yaml");
    if !video_path.exists() {
        fs::write(&video_path, DEFAULT_VIDEO_PRESETS)
            .with_context(|| format!("Cannot write {}", video_path.display()))?;
        info!("Created default video presets: {}", video_path.display());
    }

    // Generate audio_codecs.yaml
    let audio_path = config_dir.join("audio_codecs.yaml");
    if !audio_path.exists() {
        fs::write(&audio_path, DEFAULT_AUDIO_PRESETS)
            .with_context(|| format!("Cannot write {}", audio_path.display()))?;
        info!("Created default audio presets: {}", audio_path.display());
    }

    // Generate image_codecs.yaml
    let image_path = config_dir.join("image_codecs.yaml");
    if !image_path.exists() {
        fs::write(&image_path, DEFAULT_IMAGE_PRESETS)
            .with_context(|| format!("Cannot write {}", image_path.display()))?;
        info!("Created default image presets: {}", image_path.display());
    }

    // Generate pdf_presets.yaml
    let pdf_path = config_dir.join("pdf_presets.yaml");
    if !pdf_path.exists() {
        fs::write(&pdf_path, DEFAULT_PDF_PRESETS)
            .with_context(|| format!("Cannot write {}", pdf_path.display()))?;
        info!("Created default PDF presets: {}", pdf_path.display());
    }

    // Generate document_presets.yaml
    let doc_path = config_dir.join("document_presets.yaml");
    if !doc_path.exists() {
        fs::write(&doc_path, DEFAULT_DOCUMENT_PRESETS)
            .with_context(|| format!("Cannot write {}", doc_path.display()))?;
        info!("Created default document presets: {}", doc_path.display());
    }

    // Generate custom_presets.yaml
    let custom_path = config_dir.join("custom_presets.yaml");
    if !custom_path.exists() {
        fs::write(&custom_path, DEFAULT_CUSTOM_PRESETS)
            .with_context(|| format!("Cannot write {}", custom_path.display()))?;
        info!("Created default custom presets: {}", custom_path.display());
    }

    Ok(())
}

// Default preset file contents (generated on first run)
const DEFAULT_VIDEO_PRESETS: &str = r#"# Video codec presets — reference by name in watcher rules.
# Preset names match the FFmpeg codec name for clarity.

presets:
  # ── CPU Encoding ──
  libx264:
    codec: libx264
    quality: crf 23
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "libx264 H.264/AVC — crf 23, aac 128k, .mp4 — general purpose"

  libx264_high:
    codec: libx264
    quality: crf 18
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .mp4
    description: "libx264 H.264/AVC — crf 18, aac 192k, .mp4 — high quality archival"

  libx265:
    codec: libx265
    quality: crf 28
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "libx265 H.265/HEVC — crf 28, aac 128k, .mp4 — smaller files, slower"

  libx265_high:
    codec: libx265
    quality: crf 22
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .mkv
    description: "libx265 H.265/HEVC — crf 22, aac 192k, .mkv — high quality archival"

  libvpx-vp9:
    codec: libvpx-vp9
    quality: crf 30
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .webm
    description: "libvpx-vp9 VP9 — crf 30, libopus 128k, .webm — web streaming"

  libaom-av1:
    codec: libaom-av1
    quality: crf 30
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .mp4
    description: "libaom-av1 AV1 — crf 30, libopus 128k, .mp4 — best compression, very slow"

  # ── VAAPI (Intel / AMD integrated GPU) ──
  h264_vaapi:
    codec: h264_vaapi
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_vaapi H.264/AVC — qp 25, copy audio, .mp4 — Intel/AMD GPU"

  hevc_vaapi:
    codec: hevc_vaapi
    quality: qp 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_vaapi H.265/HEVC — qp 28, copy audio, .mkv — Intel/AMD GPU"

  vp9_vaapi:
    codec: vp9_vaapi
    quality: qp 28
    audio_codec: copy
    output_ext: .webm
    description: "vp9_vaapi VP9 — qp 28, copy audio, .webm — web streaming via GPU"

  av1_vaapi:
    codec: av1_vaapi
    quality: qp 30
    audio_codec: copy
    output_ext: .mp4
    description: "av1_vaapi AV1 — qp 30, copy audio, .mp4 — best compression via GPU (Arc / newer Intel)"

  # ── NVENC (NVIDIA GPU) ──
  h264_nvenc:
    codec: h264_nvenc
    quality: cq 23
    audio_codec: copy
    output_ext: .mp4
    description: "h264_nvenc H.264/AVC — cq 23, copy audio, .mp4 — NVIDIA GPU fast"

  h264_nvenc_high:
    codec: h264_nvenc
    quality: cq 18
    audio_codec: copy
    output_ext: .mp4
    description: "h264_nvenc H.264/AVC — cq 18, copy audio, .mp4 — NVIDIA GPU high quality"

  hevc_nvenc:
    codec: hevc_nvenc
    quality: cq 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_nvenc H.265/HEVC — cq 28, copy audio, .mkv — NVIDIA GPU"

  hevc_nvenc_high:
    codec: hevc_nvenc
    quality: cq 22
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_nvenc H.265/HEVC — cq 22, copy audio, .mkv — NVIDIA GPU high quality"

  av1_nvenc:
    codec: av1_nvenc
    quality: cq 28
    audio_codec: copy
    output_ext: .mp4
    description: "av1_nvenc AV1 — cq 28, copy audio, .mp4 — NVIDIA GPU (RTX 40-series+)"

  # ── AMF (AMD GPU) ──
  h264_amf:
    codec: h264_amf
    quality: qp_i 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_amf H.264/AVC — qp_i 25, copy audio, .mp4 — AMD GPU"

  hevc_amf:
    codec: hevc_amf
    quality: qp_i 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_amf H.265/HEVC — qp_i 28, copy audio, .mkv — AMD GPU"

  av1_amf:
    codec: av1_amf
    quality: qp_i 28
    audio_codec: copy
    output_ext: .mp4
    description: "av1_amf AV1 — qp_i 28, copy audio, .mp4 — AMD GPU (RX 7000+)"

  # ── QSV (Intel QuickSync via MediaSDK) ──
  h264_qsv:
    codec: h264_qsv
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_qsv H.264/AVC — qp 25, copy audio, .mp4 — Intel QuickSync"

  hevc_qsv:
    codec: hevc_qsv
    quality: qp 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_qsv H.265/HEVC — qp 28, copy audio, .mkv — Intel QuickSync"

  vp9_qsv:
    codec: vp9_qsv
    quality: qp 28
    audio_codec: copy
    output_ext: .webm
    description: "vp9_qsv VP9 — qp 28, copy audio, .webm — Intel QuickSync"

  av1_qsv:
    codec: av1_qsv
    quality: qp 30
    audio_codec: copy
    output_ext: .mp4
    description: "av1_qsv AV1 — qp 30, copy audio, .mp4 — Intel QuickSync (Arc / 12th gen+)"

  # ── VideoToolbox (macOS) ──
  h264_videotoolbox:
    codec: h264_videotoolbox
    quality: constant_bit_rate 3000
    audio_codec: copy
    output_ext: .mp4
    description: "h264_videotoolbox H.264/AVC — b:v 3000, copy audio, .mp4 — macOS VideoToolbox"

  hevc_videotoolbox:
    codec: hevc_videotoolbox
    quality: constant_bit_rate 5000
    audio_codec: copy
    output_ext: .mp4
    description: "hevc_videotoolbox H.265/HEVC — b:v 5000, copy audio, .mp4 — macOS VideoToolbox"

  # ── OMX (Raspberry Pi) ──
  h264_omx:
    codec: h264_omx
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_omx H.264/AVC — qp 25, copy audio, .mp4 — Raspberry Pi OMX (legacy; OMX removed in FFmpeg 6.x+, prefer h264_v4l2m2m)"

  # ── RKMPP (Rockchip MPP — RK3588, NanoPi R6S, etc.) ──
  h264_rkmpp:
    codec: h264_rkmpp
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_rkmpp H.264/AVC — qp 25, copy audio, .mp4 — Rockchip MPP hardware"

  hevc_rkmpp:
    codec: hevc_rkmpp
    quality: qp 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_rkmpp HEVC/HEVC — qp 28, copy audio, .mkv — Rockchip MPP hardware"

  # ── Raspberry Pi (V4L2 mem2mem) ──
  h264_v4l2m2m:
    codec: h264_v4l2m2m
    quality: 3000k
    audio_codec: aac
    output_ext: .mp4
    description: "h264_v4l2m2m H.264/AVC — V4L2 mem2mem, Raspberry Pi hardware"

  hevc_v4l2m2m:
    codec: hevc_v4l2m2m
    quality: 3000k
    audio_codec: aac
    output_ext: .mkv
    description: "hevc_v4l2m2m HEVC/H.265 — V4L2 mem2mem, Raspberry Pi 4/5 hardware"

  # ── Legacy ──
  mpeg4:
    codec: mpeg4
    quality: qscale 4
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .avi
    description: "mpeg4 MPEG-4 Part 2 — qscale 4, aac 128k, .avi — legacy compatibility"

  mpeg2video:
    codec: mpeg2video
    quality: qscale 4
    audio_codec: mp2
    audio_bitrate: 192k
    output_ext: .mpg
    description: "mpeg2video MPEG-2 — qscale 4, mp2 192k, .mpg — DVD / broadcast"

  # ── Pass-through ──
  copy:
    codec: copy
    audio_codec: copy
    output_ext: .mp4
    description: "copy — stream copy all streams, no re-encoding, remux only, .mp4"

  copy_aac:
    codec: copy
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "copy_aac — copy video stream, re-encode audio to aac 128k, .mp4"
"#;

const DEFAULT_AUDIO_PRESETS: &str = r#"# Audio codec presets — reference by name in watcher rules.

presets:
  # ── Lossy ──
  mp3_128:
    audio_codec: libmp3lame
    audio_bitrate: 128k
    output_ext: .mp3
    description: "MP3 128kbps — standard quality"

  mp3_192:
    audio_codec: libmp3lame
    audio_bitrate: 192k
    output_ext: .mp3
    description: "MP3 192kbps — good quality"

  mp3_320:
    audio_codec: libmp3lame
    audio_bitrate: 320k
    output_ext: .mp3
    description: "MP3 320kbps — maximum MP3 quality"

  aac_128:
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .m4a
    description: "AAC 128kbps — standard quality"

  aac_192:
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .m4a
    description: "AAC 192kbps — good quality"

  aac_256:
    audio_codec: aac
    audio_bitrate: 256k
    output_ext: .m4a
    description: "AAC 256kbps — high quality"

  opus_96:
    audio_codec: libopus
    audio_bitrate: 96k
    output_ext: .opus
    description: "Opus 96kbps — efficient, voice"

  opus_128:
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .opus
    description: "Opus 128kbps — efficient, general purpose"

  opus_192:
    audio_codec: libopus
    audio_bitrate: 192k
    output_ext: .opus
    description: "Opus 192kbps — efficient, music"

  vorbis_192:
    audio_codec: libvorbis
    audio_bitrate: 192k
    output_ext: .ogg
    description: "Vorbis 192kbps — open source"

  ac3_192:
    audio_codec: ac3
    audio_bitrate: 192k
    output_ext: .ac3
    description: "AC3 192kbps — Dolby Digital"

  ac3_6ch:
    audio_codec: ac3
    audio_bitrate: 448k
    channels: 6
    output_ext: .ac3
    description: "AC3 5.1 surround — 448kbps"

  # ── Lossless ──
  flac:
    audio_codec: flac
    output_ext: .flac
    description: "FLAC — lossless compression"

  alac:
    audio_codec: alac
    output_ext: .m4a
    description: "ALAC — Apple lossless"

  pcm_s16le:
    audio_codec: pcm_s16le
    output_ext: .wav
    description: "PCM 16-bit — uncompressed WAV"

  # ── Pass-through ──
  copy_audio:
    audio_codec: copy
    output_ext: .mp3
    description: "Stream copy — no re-encoding"
"#;

const DEFAULT_IMAGE_PRESETS: &str = r#"# Image format presets — reference by name in watcher rules.

presets:
  png:
    output_ext: .png
    quality: 90
    description: "PNG — lossless, supports transparency"

  jpeg_90:
    output_ext: .jpg
    quality: 90
    description: "JPEG 90% — high quality"

  jpeg_80:
    output_ext: .jpg
    quality: 80
    description: "JPEG 80% — balanced quality/size"

  jpeg_70:
    output_ext: .jpg
    quality: 70
    description: "JPEG 70% — web optimized"

  webp_90:
    output_ext: .webp
    quality: 90
    description: "WebP 90% — modern web format"

  webp_80:
    output_ext: .webp
    quality: 80
    description: "WebP 80% — balanced"

  webp_lossless:
    output_ext: .webp
    quality: 100
    description: "WebP lossless"

  avif_90:
    output_ext: .avif
    quality: 90
    description: "AVIF 90% — next-gen format, best compression"

  avif_80:
    output_ext: .avif
    quality: 80
    description: "AVIF 80% — balanced"

  heif:
    output_ext: .heic
    quality: 90
    description: "HEIF/HEIC — Apple format"

  tiff:
    output_ext: .tiff
    description: "TIFF — uncompressed, archival"

  gif:
    output_ext: .gif
    description: "GIF — animated, 256 colors"
"#;

const DEFAULT_PDF_PRESETS: &str = r#"# PDF processing presets — reference by name in watcher rules.

presets:
  # ── Compression ──
  pdf_screen:
    mode: compress
    pdf_quality: screen
    output_ext: .pdf
    description: "PDF — screen quality, smallest size (72dpi)"

  pdf_ebook:
    mode: compress
    pdf_quality: ebook
    output_ext: .pdf
    description: "PDF — ebook quality (150dpi)"

  pdf_printer:
    mode: compress
    pdf_quality: printer
    output_ext: .pdf
    description: "PDF — print quality (300dpi)"

  pdf_prepress:
    mode: compress
    pdf_quality: prepress
    output_ext: .pdf
    description: "PDF — prepress quality (300dpi, color preserved)"

  # ── PDF/A (archival) ──
  pdfa_1b:
    mode: pdf_a
    pdfa_version: "1B"
    output_ext: .pdf
    description: "PDF/A-1b — archival standard"

  pdfa_2b:
    mode: pdf_a
    pdfa_version: "2B"
    output_ext: .pdf
    description: "PDF/A-2b — modern archival"

  # ── Extraction ──
  pdf_extract_text:
    mode: extract_text
    output_ext: .txt
    description: "Extract text from PDF"

  pdf_extract_images:
    mode: extract_images
    output_ext: .png
    description: "Extract images from PDF"

  # ── Conversion ──
  image_to_pdf:
    mode: image_to_pdf
    output_ext: .pdf
    description: "Images to PDF"

  pdf_to_images:
    mode: pdf_to_images
    output_ext: .png
    resolution: 150
    description: "PDF pages to images (150dpi)"

  pdf_to_images_hd:
    mode: pdf_to_images
    output_ext: .png
    resolution: 300
    description: "PDF pages to images (300dpi)"

  # ── Optimization ──
  pdf_linearize:
    mode: linearize
    output_ext: .pdf
    description: "Fast web view (linearized PDF)"

  pdf_merge:
    mode: merge
    output_ext: .pdf
    description: "Merge multiple PDFs"

  # ── Security ──
  pdf_encrypt:
    mode: encrypt
    output_ext: .pdf
    description: "Encrypt PDF with password"

  pdf_decrypt:
    mode: decrypt
    output_ext: .pdf
    description: "Remove PDF encryption"

  # ── Analysis ──
  pdf_analyze:
    mode: analyze
    description: "Analyze PDF structure (no output file)"
"#;

const DEFAULT_DOCUMENT_PRESETS: &str = r#"# Document conversion presets — reference by name in watcher rules.

presets:
  # ── To PDF ──
  docx_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    description: "DOCX to PDF via WeasyPrint"

  md_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    toc: true
    description: "Markdown to PDF with table of contents"

  html_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    description: "HTML to PDF"

  odt_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    description: "ODT to PDF"

  epub_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    description: "EPUB to PDF"

  # ── To Markdown ──
  docx_to_md:
    output_ext: .md
    standalone: true
    description: "DOCX to Markdown"

  html_to_md:
    output_ext: .md
    standalone: true
    description: "HTML to Markdown"

  # ── To HTML ──
  docx_to_html:
    output_ext: .html
    standalone: true
    description: "DOCX to standalone HTML"

  md_to_html:
    output_ext: .html
    standalone: true
    description: "Markdown to HTML"

  # ── To EPUB ──
  docx_to_epub:
    output_ext: .epub
    description: "DOCX to EPUB"

  md_to_epub:
    output_ext: .epub
    description: "Markdown to EPUB"

  # ── To DOCX ──
  md_to_docx:
    output_ext: .docx
    description: "Markdown to DOCX"

  html_to_docx:
    output_ext: .docx
    description: "HTML to DOCX"
"#;

const DEFAULT_CUSTOM_PRESETS: &str = r#"# Custom command presets — reference by name in watcher rules.
# Define arbitrary CLI commands for file conversion.
# Placeholders: {input}, {output}, {basename}, {ext}, {output_folder}

presets:
  handbrake:
    command: "HandBrakeCLI -i {input} -o {output} --preset 'Fast 1080p30'"
    output_ext: .mp4
    description: "HandBrake CLI — Fast 1080p30 preset"

  imagemagick:
    command: "convert {input} {output}"
    output_ext: .png
    description: "ImageMagick convert"

  ghostscript_compress:
    command: "gs -sDEVICE=pdfwrite -dCompatibilityLevel=1.4 -dPDFSETTINGS=/ebook -o {output} {input}"
    output_ext: .pdf
    description: "Ghostscript PDF compression (ebook quality)"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_absolute_path_rejects_relative() {
        assert!(validate_absolute_path("/app/inputs/", "test").is_ok());
        assert!(validate_absolute_path("/usr/bin/ffmpeg", "test").is_ok());
        assert!(validate_absolute_path("./inputs/", "test").is_err());
        assert!(validate_absolute_path("../config/", "test").is_err());
        assert!(validate_absolute_path("config.yaml", "test").is_err());
    }

    fn test_global() -> GlobalConfig {
        GlobalConfig {
            log: global::LogConfig {
                errors_file: "/app/logs/errors.log".to_string(),
                ..Default::default()
            },
            history: global::HistoryConfig {
                file: "/app/logs/history.json".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_empty_watch_folder() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: String::new(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video {
                rules: vec![],
            },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("watch_folder must be an absolute path"));
    }

    #[test]
    fn test_validate_empty_output_folder() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: String::new(),
            watch_type: WatchType::Video {
                rules: vec![],
            },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("output_folder must be an absolute path"));
    }

    #[test]
    fn test_validate_empty_input_extensions() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video {
                rules: vec![watch::VideoRule {
                    preset: "libx264".to_string(),
                    subfolder: None,
                    input_extensions: vec![],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: None,
                    min_duration_ratio: None,
                }],
            },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("input_extensions must not be empty"));
    }

    #[test]
    fn test_validate_config_passes_with_valid_data() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video {
                rules: vec![watch::VideoRule {
                    preset: "libx264".to_string(),
                    subfolder: None,
                    input_extensions: vec![".mp4".into(), ".mxf".into()],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: None,
                    min_duration_ratio: None,
                }],
            },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_all_watcher_types() {
        let global = test_global();
        let watchers = vec![
            WatchConfig {
                name: "video".to_string(),
                watch_folder: "/app/inputs/video/".to_string(),
                output_folder: "/app/outputs/video/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Video {
                    rules: vec![watch::VideoRule {
                        preset: "libx264".to_string(),
                        subfolder: None,
                        input_extensions: vec![".mp4".into()],
                        output_ext: None, codec: None, quality: None,
                        audio_codec: None, audio_bitrate: None,
                        output_name: None, check_duration: None,
                        min_duration_ratio: None,
                    }],
                },
            },
            WatchConfig {
                name: "image".to_string(),
                watch_folder: "/app/inputs/image/".to_string(),
                output_folder: "/app/outputs/image/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Image {
                    rules: vec![watch::ImageRule {
                        preset: "jpeg_80".to_string(),
                        subfolder: None,
                        input_extensions: vec![".tiff".into()],
                        output_ext: None, quality: None, transparent: None,
                        output_name: None,
                    }],
                },
            },
            WatchConfig {
                name: "audio".to_string(),
                watch_folder: "/app/inputs/audio/".to_string(),
                output_folder: "/app/outputs/audio/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Audio {
                    rules: vec![watch::AudioRule {
                        preset: "mp3_192".to_string(),
                        subfolder: None,
                        input_extensions: vec![".wav".into()],
                        output_ext: None, audio_codec: None, audio_bitrate: None,
                        sample_rate: None, channels: None, output_name: None,
                    }],
                },
            },
            WatchConfig {
                name: "pdf".to_string(),
                watch_folder: "/app/inputs/pdf/".to_string(),
                output_folder: "/app/outputs/pdf/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Pdf {
                    rules: vec![watch::PdfRule {
                        preset: "pdf_ebook".to_string(),
                        subfolder: None,
                        input_extensions: vec![".pdf".into()],
                        output_ext: None, mode: None, quality: None,
                        pdfa_version: None, resolution: None, password: None,
                        output_name: None,
                    }],
                },
            },
            WatchConfig {
                name: "document".to_string(),
                watch_folder: "/app/inputs/doc/".to_string(),
                output_folder: "/app/outputs/doc/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Document {
                    rules: vec![watch::DocumentRule {
                        preset: "docx_to_pdf".to_string(),
                        subfolder: None,
                        input_extensions: vec![".docx".into()],
                        output_ext: None, toc: None, toc_depth: None,
                        css: None, template: None, standalone: None,
                        metadata: None, pdf_engine: None, options: None,
                        output_name: None,
                    }],
                },
            },
            WatchConfig {
                name: "custom".to_string(),
                watch_folder: "/app/inputs/custom/".to_string(),
                output_folder: "/app/outputs/custom/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Custom {
                    rules: vec![watch::CustomRule {
                        preset: "handbrake".to_string(),
                        subfolder: None,
                        input_extensions: vec![".mkv".into()],
                        output_ext: None, command: None, output_name: None,
                        description: None,
                    }],
                },
            },
        ];
        let result = validate_config(&global, &watchers);
        assert!(result.is_ok());
    }
}
