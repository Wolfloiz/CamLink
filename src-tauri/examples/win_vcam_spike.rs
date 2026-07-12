//! Spike B (T016) — valida o backend Windows de câmera virtual via
//! akvirtualcamera.
//!
//! **Resultado: REPROVADO** (2026-07-10) — mantido só como referência
//! histórica/diagnóstica. O driver não registra o device no SO em Windows 11
//! (backend Media Foundation com bug upstream não resolvido). Backend
//! Windows substituído por filtro DirectShow próprio — ver Spike C (T074,
//! `win_dshow_spike.rs`) e `specs/001-phone-webcam-bridge/research.md` R4.
//!
//! Carrega `libvcam_capi.dll` do driver `akvirtualcamera` (instalado
//! separadamente — <https://github.com/webcamoid/akvirtualcamera>) via FFI
//! dinâmico, cria um device de teste, registra um formato RGB24 e empurra
//! frames por alguns segundos. Cada frame carrega, em uma barra vertical de
//! 24 bits na borda esquerda, o timestamp (ms desde o início do processo) em
//! que foi enviado — a barra usa altura cheia para não depender da ordem de
//! varredura (RGB24 no DirectShow costuma ser bottom-up). Em paralelo, captura
//! o mesmo device via `ffmpeg -f dshow` e decodifica essa barra para medir a
//! latência do caminho push→consumidor (parte do orçamento decode→push de
//! FR-004/SC-002, ≤ 70 ms) sem depender de câmera/relógio físico.
//!
//! A validação em OBS/Chrome/Firefox/Discord (critério de aceite de T016)
//! ainda exige inspeção humana — este spike deixa o device no ar pelo tempo
//! configurado para isso.
//!
//! Uso: `cargo run --example win_vcam_spike --release -- [segundos]`

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("win_vcam_spike é específico do Windows (Spike B / T016).");
}

#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_spike::run()
}

#[cfg(target_os = "windows")]
mod windows_spike {
    use std::env;
    use std::ffi::{c_void, CStr, CString};
    use std::io::Read;
    use std::os::raw::{c_char, c_int};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use libloading::{Library, Symbol};

    const WIDTH: i32 = 640;
    const HEIGHT: i32 = 480;
    const FPS: i32 = 30;
    const FORMAT: &str = "RGB24";
    const DESCRIPTION: &str = "CamLink Spike B";
    const DEVICE_ID: &str = "CamLinkSpikeB0";
    const BARCODE_BITS: u32 = 24;
    const BARCODE_WIDTH: i32 = 240;
    const FRAME_SIZE: usize = (WIDTH * HEIGHT * 3) as usize;

    type VcamOpenFn = unsafe extern "C" fn() -> *mut c_void;
    type VcamCloseFn = unsafe extern "C" fn(*mut c_void);
    type VcamAddDeviceFn =
        unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_char, usize) -> c_int;
    type VcamRemoveDeviceFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
    type VcamAddFormatFn = unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        *const c_char,
        c_int,
        c_int,
        c_int,
        c_int,
        *mut c_int,
    ) -> c_int;
    type VcamUpdateFn = unsafe extern "C" fn(*mut c_void) -> c_int;
    type VcamStreamStartFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
    type VcamStreamSendFn = unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        *const c_char,
        c_int,
        c_int,
        *const *const c_char,
        *const usize,
    ) -> c_int;
    type VcamStreamStopFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
    type VcamDevicesFn = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut usize) -> c_int;
    type VcamSystemApiFn = unsafe extern "C" fn(*mut c_void, *mut c_char, *mut usize);

    /// Bindings resolvidos de `libvcam_capi.dll`. `_lib` mantém a DLL
    /// carregada; os ponteiros de função só são válidos enquanto ela existir.
    struct AkVCam<'lib> {
        handle: *mut c_void,
        add_device: Symbol<'lib, VcamAddDeviceFn>,
        remove_device: Symbol<'lib, VcamRemoveDeviceFn>,
        add_format: Symbol<'lib, VcamAddFormatFn>,
        update: Symbol<'lib, VcamUpdateFn>,
        stream_start: Symbol<'lib, VcamStreamStartFn>,
        stream_send: Symbol<'lib, VcamStreamSendFn>,
        stream_stop: Symbol<'lib, VcamStreamStopFn>,
        close: Symbol<'lib, VcamCloseFn>,
        devices: Symbol<'lib, VcamDevicesFn>,
        system_api: Symbol<'lib, VcamSystemApiFn>,
    }

    impl<'lib> AkVCam<'lib> {
        fn load(lib: &'lib Library) -> Result<Self, Box<dyn std::error::Error>> {
            // SAFETY: os nomes/assinaturas vêm de `capi/src/capi.h` do
            // akvirtualcamera; a DLL carregada é o build oficial do driver.
            unsafe {
                let open: Symbol<VcamOpenFn> = lib.get(b"vcam_open\0")?;
                let handle = open();
                if handle.is_null() {
                    return Err("vcam_open retornou NULL (driver não iniciado?)".into());
                }
                Ok(Self {
                    handle,
                    add_device: lib.get(b"vcam_add_device\0")?,
                    remove_device: lib.get(b"vcam_remove_device\0")?,
                    add_format: lib.get(b"vcam_add_format\0")?,
                    update: lib.get(b"vcam_update\0")?,
                    stream_start: lib.get(b"vcam_stream_start\0")?,
                    stream_send: lib.get(b"vcam_stream_send\0")?,
                    stream_stop: lib.get(b"vcam_stream_stop\0")?,
                    close: lib.get(b"vcam_close\0")?,
                    devices: lib.get(b"vcam_devices\0")?,
                    system_api: lib.get(b"vcam_system_api\0")?,
                })
            }
        }

        /// Pede um `device_id` fixo (em vez de deixar o driver gerar um).
        ///
        /// Bug conhecido do driver (upstream, `capi.cpp::vcam_add_device`):
        /// no Windows `add-device` é SEMPRE elevado (`needsRoot` retorna
        /// true incondicionalmente); a chamada eleva um `AkVCamManager.exe`
        /// via UAC que cria o device de fato (confirmado manualmente com
        /// `AkVCamManager devices`), mas o próprio wrapper detecta "o que
        /// mudou" comparando `m_bridge.devices()` antes/depois — uma lista
        /// cacheada em processo, atualizada só por um timer de 1000 ms
        /// (`ipcbridge.cpp: m_messagesTimer.setInterval(1000)`). Como o
        /// diff roda antes do próximo tick, `vcam_add_device` quase sempre
        /// retorna `-ENOENT` mesmo com sucesso. Por isso pedimos um ID
        /// conhecido (`-i` para o manager) e, se a chamada falhar, o
        /// chamador faz poll de `devices()` por ~1s+ até o cache atualizar
        /// (ver `run()`).
        fn add_device(&self, description: &str) -> Result<String, Box<dyn std::error::Error>> {
            let description = CString::new(description)?;
            let mut buf = [0u8; 256];
            let requested_id = CString::new(DEVICE_ID)?;
            let requested_bytes = requested_id.as_bytes_with_nul();
            buf[..requested_bytes.len()].copy_from_slice(requested_bytes);
            // SAFETY: buffer com capacidade fixa informada em `buf.len()`;
            // a DLL lê o `device_id` já preenchido (ID desejado) e devolve o
            // ID efetivo (mesmo valor) NUL-terminated no mesmo buffer.
            let rc = unsafe {
                (self.add_device)(
                    self.handle,
                    description.as_ptr(),
                    buf.as_mut_ptr().cast::<c_char>(),
                    buf.len(),
                )
            };
            if rc != 0 {
                return Err(format!("vcam_add_device falhou (rc={rc})").into());
            }
            // SAFETY: `buf` foi preenchido pela chamada acima e é NUL-terminated.
            let device_id = unsafe { CStr::from_ptr(buf.as_ptr().cast::<c_char>()) };
            Ok(device_id.to_string_lossy().into_owned())
        }

        /// Lista devices conhecidos pelo cache em processo (ver nota em
        /// `add_device` sobre o atraso de até ~1s desse cache).
        fn devices(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
            let mut buf = [0u8; 4096];
            let mut buffer_size = buf.len();
            // SAFETY: buffer com capacidade fixa informada em `buffer_size`;
            // a DLL escreve uma sequência de C strings NUL-terminated,
            // seguida de um NUL final, sem exceder o tamanho informado.
            let rc = unsafe {
                (self.devices)(
                    self.handle,
                    buf.as_mut_ptr().cast::<c_char>(),
                    &mut buffer_size,
                )
            };
            if rc < 0 {
                return Err(format!("vcam_devices falhou (rc={rc})").into());
            }
            Ok(buf[..buffer_size]
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect())
        }

        fn add_format(&self, device_id: &str) -> Result<(), Box<dyn std::error::Error>> {
            let device_id = CString::new(device_id)?;
            let format = CString::new(FORMAT)?;
            let mut index: c_int = 0;
            // SAFETY: todas as strings são C strings válidas com NUL; `index`
            // é um out-param escalar.
            let rc = unsafe {
                (self.add_format)(
                    self.handle,
                    device_id.as_ptr(),
                    format.as_ptr(),
                    WIDTH,
                    HEIGHT,
                    FPS,
                    1,
                    &mut index,
                )
            };
            if rc != 0 {
                return Err(format!("vcam_add_format falhou (rc={rc})").into());
            }
            Ok(())
        }

        fn update(&self) -> Result<(), Box<dyn std::error::Error>> {
            // SAFETY: `handle` válido, sem ponteiros adicionais.
            let rc = unsafe { (self.update)(self.handle) };
            if rc != 0 {
                return Err(format!("vcam_update falhou (rc={rc})").into());
            }
            Ok(())
        }

        fn stream_start(&self, device_id: &str) -> Result<(), Box<dyn std::error::Error>> {
            let device_id = CString::new(device_id)?;
            // SAFETY: `device_id` é uma C string válida.
            let rc = unsafe { (self.stream_start)(self.handle, device_id.as_ptr()) };
            if rc != 0 {
                return Err(format!("vcam_stream_start falhou (rc={rc})").into());
            }
            Ok(())
        }

        fn stream_send(
            &self,
            device_id: &str,
            frame: &[u8],
        ) -> Result<(), Box<dyn std::error::Error>> {
            debug_assert_eq!(frame.len(), FRAME_SIZE);
            let device_id = CString::new(device_id)?;
            let format = CString::new(FORMAT)?;
            let plane_ptr = frame.as_ptr().cast::<c_char>();
            let line_size: usize = (WIDTH * 3) as usize;
            // SAFETY: `data`/`line_size` descrevem um único plano RGB24
            // contíguo de exatamente `WIDTH*HEIGHT*3` bytes, vivo durante a
            // chamada (não é retido pela DLL após retornar).
            let rc = unsafe {
                (self.stream_send)(
                    self.handle,
                    device_id.as_ptr(),
                    format.as_ptr(),
                    WIDTH,
                    HEIGHT,
                    &plane_ptr as *const *const c_char,
                    &line_size as *const usize,
                )
            };
            if rc != 0 {
                return Err(format!("vcam_stream_send falhou (rc={rc})").into());
            }
            Ok(())
        }

        /// Nome da API de captura que o driver está usando de fato neste
        /// sistema ("DirectShow" ou "Microsoft Media Foundation") — decide
        /// se o device é visível para consumidores DirectShow (ffmpeg
        /// `-f dshow`, OBS) ou só para os baseados em Media Foundation
        /// (Chrome/Firefox/Discord/Windows Camera).
        fn system_api(&self) -> String {
            let mut buf = [0u8; 128];
            let mut len = buf.len();
            // SAFETY: buffer com capacidade fixa informada em `len`; a DLL
            // escreve uma C string NUL-terminated sem exceder o tamanho.
            unsafe { (self.system_api)(self.handle, buf.as_mut_ptr().cast::<c_char>(), &mut len) };
            unsafe { CStr::from_ptr(buf.as_ptr().cast::<c_char>()) }
                .to_string_lossy()
                .into_owned()
        }

        fn stream_stop(&self, device_id: &str) {
            if let Ok(device_id) = CString::new(device_id) {
                // SAFETY: `handle`/`device_id` válidos; erro de shutdown não
                // é fatal para o spike, só logado.
                let rc = unsafe { (self.stream_stop)(self.handle, device_id.as_ptr()) };
                if rc != 0 {
                    eprintln!("aviso: vcam_stream_stop rc={rc}");
                }
            }
        }

        fn remove_device(&self, device_id: &str) {
            if let Ok(device_id) = CString::new(device_id) {
                // SAFETY: `handle`/`device_id` válidos.
                let rc = unsafe { (self.remove_device)(self.handle, device_id.as_ptr()) };
                if rc != 0 {
                    eprintln!("aviso: vcam_remove_device rc={rc}");
                }
            }
        }
    }

    impl<'lib> Drop for AkVCam<'lib> {
        fn drop(&mut self) {
            // SAFETY: `handle` só é fechado uma vez, aqui.
            unsafe { (self.close)(self.handle) };
        }
    }

    fn locate_dll() -> PathBuf {
        if let Ok(path) = env::var("CAMLINK_AKVCAM_DLL") {
            return PathBuf::from(path);
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(base) = env::var(var) {
                let candidate = Path::new(&base)
                    .join("akvirtualcamera")
                    .join("x64")
                    .join("libvcam_capi.dll");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        // Último recurso: espera que esteja no PATH.
        PathBuf::from("libvcam_capi.dll")
    }

    /// Codifica `elapsed_ms` como uma barra de `BARCODE_BITS` blocos
    /// preto/branco de altura cheia (robusto a inversão vertical do DIB) e
    /// preenche o restante do frame com barras de cor estáticas, só para
    /// checagem visual de que o device está realmente exibindo vídeo.
    fn make_frame(elapsed_ms: u32, buf: &mut [u8]) {
        const BARS: [[u8; 3]; 7] = [
            [235, 235, 235],
            [235, 235, 16],
            [16, 235, 235],
            [16, 235, 16],
            [235, 16, 235],
            [235, 16, 16],
            [16, 16, 235],
        ];
        let bar_width = BARCODE_WIDTH / BARCODE_BITS as i32;
        let color_width = (WIDTH - BARCODE_WIDTH) / BARS.len() as i32;

        for y in 0..HEIGHT {
            let row = (y * WIDTH * 3) as usize;
            for x in 0..WIDTH {
                let px = row + (x * 3) as usize;
                let color = if x < BARCODE_WIDTH {
                    let bit_idx = (x / bar_width.max(1)) as u32;
                    let bit = (elapsed_ms >> (BARCODE_BITS.saturating_sub(1 + bit_idx))) & 1;
                    if bit == 1 {
                        [255u8, 255, 255]
                    } else {
                        [0u8, 0, 0]
                    }
                } else {
                    let band =
                        (((x - BARCODE_WIDTH) / color_width.max(1)) as usize).min(BARS.len() - 1);
                    BARS[band]
                };
                buf[px] = color[0];
                buf[px + 1] = color[1];
                buf[px + 2] = color[2];
            }
        }
    }

    /// Decodifica a barra da §`make_frame` a partir de um frame capturado
    /// (linha do meio, para tolerar flip vertical).
    fn decode_barcode(frame: &[u8]) -> u32 {
        let bar_width = BARCODE_WIDTH / BARCODE_BITS as i32;
        let y = HEIGHT / 2;
        let row = (y * WIDTH * 3) as usize;
        let mut value: u32 = 0;
        for bit_idx in 0..BARCODE_BITS {
            let x = bit_idx as i32 * bar_width + bar_width / 2;
            let px = row + (x * 3) as usize;
            let luminance = (frame[px] as u32 + frame[px + 1] as u32 + frame[px + 2] as u32) / 3;
            let bit = if luminance > 128 { 1 } else { 0 };
            value = (value << 1) | bit;
        }
        value
    }

    /// Captura o device via `ffmpeg -f dshow` em uma thread separada e
    /// decodifica a latência push→captura de cada frame recebido.
    fn spawn_latency_probe(
        friendly_name: String,
        t0: Instant,
        max_frames: usize,
    ) -> mpsc::Receiver<Result<Vec<u64>, String>> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| -> Result<Vec<u64>, String> {
                let device_arg = format!("video={friendly_name}");
                let mut child = Command::new("ffmpeg")
                    .args([
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-f",
                        "dshow",
                        "-video_size",
                        &format!("{WIDTH}x{HEIGHT}"),
                        "-framerate",
                        &FPS.to_string(),
                        "-pixel_format",
                        "rgb24",
                        "-i",
                        &device_arg,
                        "-pix_fmt",
                        "rgb24",
                        "-f",
                        "rawvideo",
                        "-",
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("não foi possível iniciar ffmpeg: {e}"))?;

                let mut stderr = child.stderr.take().expect("stderr piped");
                thread::spawn(move || {
                    let mut buf = String::new();
                    let _ = stderr.read_to_string(&mut buf);
                    if !buf.trim().is_empty() {
                        eprintln!("[ffmpeg] {}", buf.trim());
                    }
                });

                let mut stdout = child.stdout.take().expect("stdout piped");
                let mut frame = vec![0u8; FRAME_SIZE];
                let mut latencies = Vec::with_capacity(max_frames);
                for _ in 0..max_frames {
                    if stdout.read_exact(&mut frame).is_err() {
                        break;
                    }
                    let captured_at_ms = t0.elapsed().as_millis() as u32;
                    let sent_at_ms = decode_barcode(&frame);
                    latencies.push(captured_at_ms.saturating_sub(sent_at_ms) as u64);
                }
                let _ = child.kill();
                let _ = child.wait();
                Ok(latencies)
            })();
            let _ = tx.send(result);
        });
        rx
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let duration_secs: u64 = env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);

        let dll_path = locate_dll();
        println!("Carregando {}", dll_path.display());
        // SAFETY: carregamos a DLL oficial do driver akvirtualcamera; ela
        // expõe símbolos C estáveis (`capi/src/capi.h`) sem inicialização
        // global além de `vcam_open`.
        let lib = unsafe { Library::new(&dll_path) }
            .map_err(|e| format!("falha ao carregar {}: {e}. Instale o akvirtualcamera (https://github.com/webcamoid/akvirtualcamera).", dll_path.display()))?;

        // Fase 1: handle descartável, só para pedir a criação do device.
        {
            let vcam = AkVCam::load(&lib)?;
            // Limpa um device de execução anterior com o mesmo ID fixo, se existir.
            vcam.remove_device(DEVICE_ID);
            if let Err(e) = vcam.add_device(DESCRIPTION) {
                // Bug conhecido do driver: ver nota em `AkVCam::add_device`.
                // O device pode ter sido criado mesmo assim (o wrapper eleva
                // um AkVCamManager.exe que faz a criação de fato); o handle
                // atual não vai enxergar isso (ver nota abaixo), então só
                // registramos o aviso e seguimos para a Fase 2.
                eprintln!("aviso: {e} — verificando via handle novo…");
            }
        } // vcam_close aqui — necessário reabrir: ver nota abaixo.

        // Fase 2: reabre um handle novo. `vcam_open` aloca um `VCamAPI` (logo
        // um `IpcBridge`) novo, cujo construtor lê o estado persistido
        // (Preferences) na hora — só assim o device criado na Fase 1 aparece.
        // Reusar o MESMO handle não funciona: `IpcBridge::devices()` só
        // devolve uma lista em cache (`m_devices`), preenchida uma única vez
        // na construção; ela deveria ser re-sincronizada a cada 1s por
        // `IpcBridgePrivate::checkStatus`, mas essa função tem um bug
        // upstream — compara `self->self->devices()` (que já É `m_devices`)
        // contra `self->m_devices`, ou seja, compara a lista consigo mesma e
        // a atualização nunca dispara. Por isso o polling em `devices()` do
        // mesmo handle nunca confirmava o device (não era uma corrida de
        // tempo, era estrutural), e `vcam_stream_send` — que valida o device
        // contra essa mesma lista em cache — sempre retornava -22 (EINVAL).
        let vcam = AkVCam::load(&lib)?;
        let device_id = DEVICE_ID.to_string();
        let mut confirmed = vcam.devices()?.iter().any(|d| d == DEVICE_ID);
        for _ in 0..8 {
            if confirmed {
                break;
            }
            thread::sleep(Duration::from_millis(500));
            confirmed = vcam.devices()?.iter().any(|d| d == DEVICE_ID);
        }
        if !confirmed {
            eprintln!(
                "aviso: device {DEVICE_ID} não confirmado no handle novo após ~4s; \
                 prosseguindo mesmo assim."
            );
        }
        println!("Device criado: {device_id} (\"{DESCRIPTION}\")");
        println!("API de captura ativa no driver: {}", vcam.system_api());
        vcam.add_format(&device_id)?;
        vcam.update()?;
        vcam.stream_start(&device_id)?;
        println!(
            "Streaming {WIDTH}x{HEIGHT}@{FPS} por {duration_secs}s. Abra OBS/Chrome/Firefox/\
             Discord e selecione \"{DESCRIPTION}\" para validar manualmente (critério de \
             aceite de T016)."
        );

        let t0 = Instant::now();
        let probe_frames = (duration_secs.saturating_sub(2) * FPS as u64).max(1) as usize;
        let probe_rx = spawn_latency_probe(DESCRIPTION.to_string(), t0, probe_frames.min(600));

        let frame_period = Duration::from_secs_f64(1.0 / FPS as f64);
        let mut frame = vec![0u8; FRAME_SIZE];
        let deadline = t0 + Duration::from_secs(duration_secs);
        let mut sent = 0u64;
        while Instant::now() < deadline {
            let loop_start = Instant::now();
            let elapsed_ms = t0.elapsed().as_millis() as u32;
            make_frame(elapsed_ms, &mut frame);
            if let Err(e) = vcam.stream_send(&device_id, &frame) {
                eprintln!("erro ao enviar frame {sent}: {e}");
                break;
            }
            sent += 1;
            let spent = loop_start.elapsed();
            if spent < frame_period {
                thread::sleep(frame_period - spent);
            }
        }
        println!("{sent} frames enviados.");

        vcam.stream_stop(&device_id);
        vcam.remove_device(&device_id);

        match probe_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(latencies)) if !latencies.is_empty() => {
                let n = latencies.len() as u64;
                let sum: u64 = latencies.iter().sum();
                let avg = sum / n;
                let min = *latencies.iter().min().unwrap();
                let max = *latencies.iter().max().unwrap();
                println!(
                    "Latência push→captura (ffmpeg dshow, {n} amostras): min={min}ms \
                     avg={avg}ms max={max}ms (orçamento FR-004/SC-002: 70ms)"
                );
                if max > 70 {
                    println!(
                        "ATENÇÃO: latência máxima excedeu 70ms — critério de aceite de T016 \
                         pede pausa e emenda à spec antes da Phase 3 no Windows."
                    );
                }
            }
            Ok(Ok(_)) => println!(
                "Sonda ffmpeg não capturou frames (device pode não ter sido enumerado a tempo) \
                 — valide manualmente em OBS/Chrome/Firefox/Discord."
            ),
            Ok(Err(e)) => println!(
                "Sonda automática de latência falhou ({e}) — valide manualmente em \
                 OBS/Chrome/Firefox/Discord e meça a latência com um relógio físico."
            ),
            Err(_) => {
                println!("Sonda automática de latência não respondeu a tempo — valide manualmente.")
            }
        }

        Ok(())
    }
}
