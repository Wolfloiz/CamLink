//! Backend Windows: filtro DirectShow próprio (push source), consolidando a
//! Spike C validada (T074, `examples/win_dshow_spike.rs`) — remove a
//! instrumentação `dbg_log` e substitui o padrão de teste auto-gerado por
//! frames reais entregues via `VirtualCameraBackend::feed_frame`.
//!
//! ## Por que memória compartilhada
//! Um filtro DirectShow "push source" roda DENTRO do processo consumidor
//! (`obs64.exe`, `chrome.exe`, ...) depois que o COM carrega este DLL via
//! `InprocServer32` — nunca dentro do processo do CamLink. A Spike C só
//! funcionou sem essa ponte porque gerava um padrão de teste sozinha,
//! internamente; frames reais precisam atravessar de processo para
//! processo. `IMemInputPin`/`IMemAllocator` são interfaces "fast path" sem
//! proxy/stub COM registrado (não marshaláveis — ver nota em `Inner`),
//! então RPC/GIT não servem aqui; o mecanismo usado por câmeras virtuais
//! reais (OBS Virtual Camera, akvirtualcamera) é o mesmo adotado aqui:
//! memória compartilhada nomeada (`FrameHub`), escrita pelo processo do
//! CamLink (`DShowBackend`, produtor) e lida por uma worker thread rodando
//! dentro do processo consumidor (`Inner`/`VCamFilter`, leitora).
//!
//! Se o leitor não vir uma nova `sequence` por `STALE_TIMEOUT`, ele mesmo
//! sintetiza uma imagem de espera local (`crate::virtualcam::standby_frame`)
//! — o consumidor nunca trava mesmo se o processo do CamLink cair, reforço
//! de FR-006 além do que a Spike C cobria (lá não havia processo externo).
//!
//! ## Limitação conhecida (v1 / US1)
//! Suporta exatamente UMA câmera virtual DirectShow por vez, com CLSID e
//! nomes de memória compartilhada fixos. Multi-fonte simultânea (US6)
//! exigiria registrar um CLSID por instância dinamicamente — não
//! implementado aqui; `create()` retorna erro se chamado com uma câmera já
//! ativa. Resolução máxima: `FRAME_MAX_WIDTH`x`FRAME_MAX_HEIGHT`.
//!
//! ## Validação
//! A parte de COM (registro, pinos, negociação, entrega de amostra) é a
//! mesma já validada em OBS/Chrome/Meet nesta sessão (research.md R4/T074).
//! A ponte de memória compartilhada e a resolução dinâmica são NOVAS nesta
//! consolidação e ainda não passaram por um ciclo real de teste em
//! OBS — precisam da mesma validação hands-on que achou os bugs #4/#5 da
//! Spike C antes de serem consideradas confiáveis.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use uuid::Uuid;
use windows::core::{
    implement, IUnknown, Interface, Ref, Result as WinResult, BOOL, GUID, HRESULT, PCWSTR,
};

use windows::Win32::Foundation::{
    CloseHandle, GetLastError, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER, E_UNEXPECTED,
    HANDLE, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::Foundation::{SIZE, WIN32_ERROR};
use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;
use windows::Win32::Media::DirectShow::{
    IAMStreamConfig, IAMStreamConfig_Impl, IBaseFilter, IBaseFilter_Impl, IEnumMediaTypes,
    IEnumMediaTypes_Impl, IEnumPins, IEnumPins_Impl, IFilterGraph, IMediaFilter, IMediaFilter_Impl,
    IMemAllocator, IMemInputPin, IPin, IPin_Impl, ALLOCATOR_PROPERTIES, FILTER_INFO, FILTER_STATE,
    PINDIR_OUTPUT, PIN_DIRECTION, PIN_INFO, VIDEO_STREAM_CONFIG_CAPS,
};
use windows::Win32::Media::IReferenceClock;
use windows::Win32::Media::KernelStreaming::{IKsPropertySet, IKsPropertySet_Impl};
use windows::Win32::Media::MediaFoundation::{
    FORMAT_VideoInfo, MEDIATYPE_Video, AM_MEDIA_TYPE, MEDIASUBTYPE_RGB24, VIDEOINFOHEADER,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemAlloc, CoTaskMemFree, CoUninitialize, IClassFactory,
    IClassFactory_Impl, IPersist, IPersist_Impl, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateMutexW, ReleaseMutex, WaitForSingleObject, INFINITE,
};

use crate::model::{VirtualCamera, VirtualCameraState};
use crate::virtualcam::{standby_frame, VcamError, VirtualCameraBackend};

// ---------------------------------------------------------------------------
// Identidade do filtro
// ---------------------------------------------------------------------------

const CLSID_CAMLINK_FILTER: GUID = GUID::from_u128(0xe7a04118_a6bc_496d_9325_c0e2d265a006);
const FILTER_FRIENDLY_NAME: &str = "CamLink Android";

/// `CLSID_VideoInputDeviceCategory` (uuids.h).
const CLSID_VIDEO_INPUT_DEVICE_CATEGORY: GUID =
    GUID::from_u128(0x860bb310_5d01_11d0_bd3b_00a0c911ce86);
/// `PIN_CATEGORY_CAPTURE` (ksmedia.h).
const PIN_CATEGORY_CAPTURE: GUID = GUID::from_u128(0xfb6c4281_0353_11d1_905f_0000c0cc16ba);
/// `AMPROPSETID_Pin` (amvideo.h / ksmedia.h).
const AMPROPSETID_PIN: GUID = GUID::from_u128(0x9b00f101_1567_11d1_b3f1_00aa003761c5);
const AMPROPERTY_PIN_CATEGORY: u32 = 0;

/// Resolução usada antes de qualquer `create()` ter rodado (ex.: um
/// consumidor consulta os formatos suportados antes do CamLink iniciar o
/// stream) ou se a ponte de memória compartilhada não puder ser aberta.
const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 720;
const DEFAULT_FPS: u32 = 30;

/// Limite v1 (ver limitação documentada no topo do arquivo).
const FRAME_MAX_WIDTH: u32 = 1920;
const FRAME_MAX_HEIGHT: u32 = 1080;
const FRAME_MAX_BYTES: usize = (FRAME_MAX_WIDTH * FRAME_MAX_HEIGHT * 3) as usize;

const STALE_TIMEOUT: Duration = Duration::from_secs(2);

fn rgb24_len(width: u32, height: u32) -> usize {
    width as usize * height as usize * 3
}

// ---------------------------------------------------------------------------
// FrameHub — canal de memória compartilhada entre o processo do CamLink
// (escritor) e o processo consumidor onde este DLL foi carregado (leitor).
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct SharedHeader {
    width: u32,
    height: u32,
    fps: u32,
    frame_len: u32,
    sequence: u64,
    live: u32,
}

const HEADER_SIZE: usize = size_of::<SharedHeader>();
const REGION_SIZE: usize = HEADER_SIZE + FRAME_MAX_BYTES;

const SHM_NAME: &str = "Local\\CamLink_VCam_Primary_Data";
const MUTEX_NAME: &str = "Local\\CamLink_VCam_Primary_Mutex";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ponte de memória compartilhada nomeada. `connect()` cria os objetos do
/// SO na primeira chamada (de qualquer um dos dois lados) e apenas abre um
/// handle novo nas chamadas seguintes — nomes de objeto kernel são
/// idempotentes, então não precisa de lógica separada create-vs-open.
struct FrameHub {
    mapping: HANDLE,
    view: *mut u8,
    mutex: HANDLE,
}

// SAFETY: `view` aponta para uma seção de memória compartilhada pelo SO;
// todo acesso de leitura/escrita passa pelo mutex nomeado (`mutex`), nunca
// concorrente sem sincronização.
unsafe impl Send for FrameHub {}
unsafe impl Sync for FrameHub {}

impl FrameHub {
    fn connect() -> WinResult<Self> {
        unsafe {
            let name = wide(SHM_NAME);
            let mapping = CreateFileMappingW(
                HANDLE::default(),
                None,
                PAGE_READWRITE,
                0,
                REGION_SIZE as u32,
                PCWSTR(name.as_ptr()),
            )?;
            let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, REGION_SIZE);
            if view.Value.is_null() {
                let _ = CloseHandle(mapping);
                return Err(windows::core::Error::from(HRESULT::from_win32(
                    GetLastError().0,
                )));
            }
            let mutex_name = wide(MUTEX_NAME);
            let mutex = CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr()))?;
            Ok(Self {
                mapping,
                view: view.Value as *mut u8,
                mutex,
            })
        }
    }

    fn header_ptr(&self) -> *mut SharedHeader {
        self.view as *mut SharedHeader
    }

    fn data_ptr(&self) -> *mut u8 {
        // SAFETY: REGION_SIZE = HEADER_SIZE + FRAME_MAX_BYTES, garantido na
        // criação da seção (`connect`); o offset fica sempre dentro do view.
        unsafe { self.view.add(HEADER_SIZE) }
    }

    /// Escreve um frame RGB24 (produtor: processo do CamLink).
    fn write_frame(&self, width: u32, height: u32, fps: u32, rgb24: &[u8], live: bool) {
        unsafe {
            let _ = WaitForSingleObject(self.mutex, INFINITE);
            let len = rgb24.len().min(FRAME_MAX_BYTES);
            core::ptr::copy_nonoverlapping(rgb24.as_ptr(), self.data_ptr(), len);
            let seq = (*self.header_ptr()).sequence.wrapping_add(1);
            self.header_ptr().write(SharedHeader {
                width,
                height,
                fps,
                frame_len: len as u32,
                sequence: seq,
                live: u32::from(live),
            });
            let _ = ReleaseMutex(self.mutex);
        }
    }

    /// Lê o cabeçalho atual sem consumir/copiar o frame (usado para
    /// negociação de media type, onde só width/height/fps importam).
    fn peek_header(&self) -> SharedHeader {
        unsafe {
            let _ = WaitForSingleObject(self.mutex, INFINITE);
            let header = *self.header_ptr();
            let _ = ReleaseMutex(self.mutex);
            header
        }
    }

    /// Copia o frame para `out` se `sequence` avançou desde `last_seen`;
    /// retorna o novo cabeçalho (leitor: worker thread dentro do processo
    /// consumidor).
    fn read_if_newer(&self, last_seen: u64, out: &mut Vec<u8>) -> Option<SharedHeader> {
        unsafe {
            let _ = WaitForSingleObject(self.mutex, INFINITE);
            let header = *self.header_ptr();
            if header.sequence == last_seen || header.frame_len == 0 {
                let _ = ReleaseMutex(self.mutex);
                return None;
            }
            out.clear();
            out.extend_from_slice(core::slice::from_raw_parts(
                self.data_ptr(),
                header.frame_len as usize,
            ));
            let _ = ReleaseMutex(self.mutex);
            Some(header)
        }
    }
}

impl Drop for FrameHub {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view as *mut c_void,
            });
            let _ = CloseHandle(self.mapping);
            let _ = CloseHandle(self.mutex);
        }
    }
}

// ---------------------------------------------------------------------------
// AM_MEDIA_TYPE — resolução/fps dinâmicos (lidos da memória compartilhada
// no momento da negociação; ver módulo doc).
// ---------------------------------------------------------------------------

fn current_video_params() -> (u32, u32, u32) {
    match FrameHub::connect() {
        Ok(hub) => {
            let h = hub.peek_header();
            if h.width > 0 && h.height > 0 {
                (h.width, h.height, h.fps.max(1))
            } else {
                (DEFAULT_WIDTH, DEFAULT_HEIGHT, DEFAULT_FPS)
            }
        }
        Err(_) => (DEFAULT_WIDTH, DEFAULT_HEIGHT, DEFAULT_FPS),
    }
}

fn build_media_type_for(width: u32, height: u32, fps: u32) -> AM_MEDIA_TYPE {
    let frame_size = rgb24_len(width, height);
    unsafe {
        let cb = size_of::<VIDEOINFOHEADER>();
        let pb = CoTaskMemAlloc(cb) as *mut u8;
        assert!(!pb.is_null(), "CoTaskMemAlloc falhou");
        let vih = pb as *mut VIDEOINFOHEADER;
        core::ptr::write_bytes(vih, 0, 1);
        (*vih).AvgTimePerFrame = 10_000_000 / i64::from(fps.max(1));
        (*vih).bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        (*vih).bmiHeader.biWidth = width as i32;
        (*vih).bmiHeader.biHeight = height as i32; // positivo = bottom-up (DIB padrão)
        (*vih).bmiHeader.biPlanes = 1;
        (*vih).bmiHeader.biBitCount = 24;
        (*vih).bmiHeader.biCompression = 0; // BI_RGB
        (*vih).bmiHeader.biSizeImage = frame_size as u32;

        AM_MEDIA_TYPE {
            majortype: MEDIATYPE_Video,
            subtype: MEDIASUBTYPE_RGB24,
            bFixedSizeSamples: BOOL::from(true),
            bTemporalCompression: BOOL::from(false),
            lSampleSize: frame_size as u32,
            formattype: FORMAT_VideoInfo,
            pUnk: core::mem::ManuallyDrop::new(None),
            cbFormat: cb as u32,
            pbFormat: pb,
        }
    }
}

fn build_media_type() -> AM_MEDIA_TYPE {
    let (w, h, fps) = current_video_params();
    build_media_type_for(w, h, fps)
}

fn free_media_type_format(mt: &AM_MEDIA_TYPE) {
    unsafe {
        if !mt.pbFormat.is_null() {
            CoTaskMemFree(Some(mt.pbFormat as *const c_void));
        }
    }
}

fn media_type_matches(mt: &AM_MEDIA_TYPE) -> bool {
    mt.majortype == MEDIATYPE_Video
        && mt.subtype == MEDIASUBTYPE_RGB24
        && mt.formattype == FORMAT_VideoInfo
}

// ---------------------------------------------------------------------------
// Estado compartilhado entre o filtro e seu único pino de saída (dentro do
// processo consumidor).
// ---------------------------------------------------------------------------

struct Inner {
    state: Mutex<FILTER_STATE>,
    peer: Mutex<Option<IPin>>,
    /// `IMemInputPin`/`IMemAllocator` do downstream. Ponteiro cru
    /// compartilhado com a worker thread — não via Global Interface Table
    /// (ver doc do módulo: essas interfaces não são marshaláveis). Acesso
    /// sempre sob o `Mutex` correspondente.
    mem_input: Mutex<Option<IMemInputPin>>,
    allocator: Mutex<Option<IMemAllocator>>,
    running: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// Referência contada (AddRef'd) do filtro dono, exigida por
    /// `IPin::QueryPinInfo::pFilter` — nunca `None` depois da criação.
    self_filter: Mutex<Option<IBaseFilter>>,
    /// Ponteiro NÃO contado para o `IFilterGraph` (convenção DirectShow:
    /// evita ciclo grafo→filtro→grafo).
    graph: Mutex<Option<*mut c_void>>,
    /// Referência contada a nós mesmos como `IPin` — `ReceiveConnection`
    /// downstream exige `pConnector` não-nulo.
    self_pin: Mutex<Option<IPin>>,
}

// SAFETY: os ponteiros COM guardados aqui só são acessados sob o Mutex
// correspondente; o filtro se registra com ThreadingModel=Both, então a
// serialização entre a thread do grafo e a worker thread é responsabilidade
// nossa (garantida pelos Mutex), não do COM.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Inner {
    fn new() -> Self {
        Self {
            state: Mutex::new(FILTER_STATE(0)),
            peer: Mutex::new(None),
            mem_input: Mutex::new(None),
            allocator: Mutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
            self_filter: Mutex::new(None),
            graph: Mutex::new(None),
            self_pin: Mutex::new(None),
        }
    }

    /// Fontes "ao vivo" devem começar a produzir amostras já em `Paused`
    /// (o Filter Graph Manager espera puxar a primeira amostra durante
    /// Stopped→Paused para concluir a transição — um source que só produz
    /// em `Run()` deixa consumidores travados em preroll com tela preta).
    fn ensure_producing(self: &Arc<Self>, state: FILTER_STATE) {
        *self.state.lock().unwrap() = state;
        if self.running.swap(true, Ordering::SeqCst) {
            return; // já rodando
        }
        let inner = Arc::clone(self);
        let handle = thread::spawn(move || worker_loop(inner));
        *self.worker.lock().unwrap() = Some(handle);
    }

    fn stop_running(self: &Arc<Self>) {
        *self.state.lock().unwrap() = FILTER_STATE(0); // State_Stopped
        self.running.store(false, Ordering::SeqCst);
        // Sem join() aqui de propósito: Stop() roda na thread do chamador
        // (ex.: a UI/STA da OBS); um join() bloqueante já causou travamento
        // permanente do encerramento da OBS numa iteração anterior desta
        // spike (ver research.md R4). A worker thread percebe
        // `running=false` sozinha e sai; o `JoinHandle` órfão só é dropado.
        self.worker.lock().unwrap().take();
    }
}

fn worker_loop(inner: Arc<Inner>) {
    // SAFETY: toda thread que participa de COM precisa se inicializar antes
    // de qualquer chamada COM, mesmo chamadas "cruas" sem marshaling como
    // as feitas aqui (ver nota em `Inner` sobre mem_input/allocator).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let hub = FrameHub::connect().ok();
    let mem_input = inner.mem_input.lock().unwrap().clone();
    let allocator = inner.allocator.lock().unwrap().clone();

    if let (Some(mem_input), Some(allocator)) = (mem_input, allocator) {
        let mut resolution = (DEFAULT_WIDTH, DEFAULT_HEIGHT);
        let mut fps = DEFAULT_FPS;
        let mut last_seq = 0u64;
        // Força a geração de uma imagem de espera já na primeira iteração,
        // antes de qualquer frame real ter chegado (preroll).
        let mut last_update = Instant::now() - STALE_TIMEOUT - Duration::from_secs(1);
        let mut buf: Vec<u8> = Vec::new();
        let mut iter: i64 = 0;

        while inner.running.load(Ordering::SeqCst) {
            let loop_start = Instant::now();
            let frame_period = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));

            let mut have_frame = false;
            if let Some(hub) = hub.as_ref() {
                if let Some(header) = hub.read_if_newer(last_seq, &mut buf) {
                    last_seq = header.sequence;
                    resolution = (header.width, header.height);
                    fps = header.fps.max(1);
                    last_update = Instant::now();
                    have_frame = true;
                }
            }
            if !have_frame && last_update.elapsed() > STALE_TIMEOUT {
                buf = standby_frame(resolution, "Aguardando CamLink...");
                convert_rgba_to_rgb24_in_place(&mut buf);
                have_frame = true;
            }

            if have_frame {
                push_sample(&mem_input, &allocator, &buf, resolution, fps, iter);
                iter += 1;
            }

            let spent = loop_start.elapsed();
            if spent < frame_period {
                thread::sleep(frame_period - spent);
            }
        }
    }

    // SAFETY: contrabalança o CoInitializeEx no início desta thread.
    unsafe {
        CoUninitialize();
    }
}

/// `standby_frame` gera RGBA (contrato do backend — ver `virtualcam::mod`);
/// o pino só transporta RGB24 (media type negociado). Descarta o canal
/// alfa in-place para não alocar um segundo buffer a cada troca para
/// standby.
fn convert_rgba_to_rgb24_in_place(buf: &mut Vec<u8>) {
    let pixels = buf.len() / 4;
    for i in 0..pixels {
        buf[i * 3] = buf[i * 4];
        buf[i * 3 + 1] = buf[i * 4 + 1];
        buf[i * 3 + 2] = buf[i * 4 + 2];
    }
    buf.truncate(pixels * 3);
}

fn push_sample(
    mem_input: &IMemInputPin,
    allocator: &IMemAllocator,
    rgb24: &[u8],
    resolution: (u32, u32),
    fps: u32,
    iter: i64,
) {
    let expected = rgb24_len(resolution.0, resolution.1);
    if rgb24.len() != expected {
        // Frame não bate com a resolução negociada (ex.: chegou durante uma
        // troca de resolução) — melhor pular a amostra do que corromper o
        // buffer do allocator.
        return;
    }
    unsafe {
        let mut sample = None;
        let hr = allocator.GetBuffer(&mut sample, None, None, 0);
        if hr.is_err() {
            return;
        }
        let Some(sample) = sample else { return };
        if let Ok(ptr) = sample.GetPointer() {
            let len = (sample.GetSize() as usize).min(rgb24.len());
            core::ptr::copy_nonoverlapping(rgb24.as_ptr(), ptr, len);
        }
        let _ = sample.SetActualDataLength(expected as i32);
        let _ = sample.SetSyncPoint(true);
        // Sem timestamp, `IMediaSample::GetTime` falha e consumidores que só
        // encaminham o frame pro pipeline de vídeo real quando têm um tempo
        // válido (ex.: `HDevice::Receive` da OBS) descartam a amostra
        // silenciosamente (bug #5 da Spike C — research.md R4).
        let frame_duration_rt: i64 = 10_000_000 / i64::from(fps.max(1));
        let start_rt = iter * frame_duration_rt;
        let end_rt = start_rt + frame_duration_rt;
        let _ = sample.SetTime(Some(&start_rt), Some(&end_rt));
        let _ = mem_input.Receive(&sample);
    }
}

// ---------------------------------------------------------------------------
// IEnumMediaTypes — só temos um formato (na resolução negociada atual).
// ---------------------------------------------------------------------------

#[implement(IEnumMediaTypes)]
struct SingleMediaTypeEnum {
    done: Mutex<bool>,
}

impl IEnumMediaTypes_Impl for SingleMediaTypeEnum_Impl {
    fn Next(
        &self,
        cmediatypes: u32,
        ppmediatypes: *mut *mut AM_MEDIA_TYPE,
        pcfetched: *mut u32,
    ) -> HRESULT {
        unsafe {
            let mut done = self.done.lock().unwrap();
            if *done || cmediatypes == 0 {
                if !pcfetched.is_null() {
                    *pcfetched = 0;
                }
                return S_FALSE;
            }
            *done = true;
            let mt = build_media_type();
            let dst = CoTaskMemAlloc(size_of::<AM_MEDIA_TYPE>()) as *mut AM_MEDIA_TYPE;
            dst.write(mt);
            *ppmediatypes = dst;
            if !pcfetched.is_null() {
                *pcfetched = 1;
            }
            // Convenção COM: S_OK só quando fetched == celt pedido.
            if cmediatypes == 1 {
                S_OK
            } else {
                S_FALSE
            }
        }
    }
    fn Skip(&self, cmediatypes: u32) -> WinResult<()> {
        if cmediatypes == 0 {
            return Ok(());
        }
        // Contrato COM: Skip precisa consumir posições de verdade, senão um
        // chamador que sonda o fim da lista via Reset+Skip(i)+Next nunca vê
        // Next falhar (bug #4 da Spike C, causava travamento permanente do
        // OBS — research.md R4). Só 1 item: qualquer Skip(>=1) já esgota.
        let mut done = self.done.lock().unwrap();
        let remaining: u32 = if *done { 0 } else { 1 };
        *done = true;
        if cmediatypes > remaining {
            Err(S_FALSE.into())
        } else {
            Ok(())
        }
    }
    fn Reset(&self) -> WinResult<()> {
        *self.done.lock().unwrap() = false;
        Ok(())
    }
    fn Clone(&self) -> WinResult<IEnumMediaTypes> {
        let done = *self.done.lock().unwrap();
        let e: IEnumMediaTypes = SingleMediaTypeEnum {
            done: Mutex::new(done),
        }
        .into();
        Ok(e)
    }
}

// ---------------------------------------------------------------------------
// IEnumPins — só temos um pino.
// ---------------------------------------------------------------------------

#[implement(IEnumPins)]
struct SinglePinEnum {
    pin: IPin,
    index: Mutex<usize>,
}

impl IEnumPins_Impl for SinglePinEnum_Impl {
    fn Next(&self, cpins: u32, pppins: *mut Option<IPin>, pcfetched: *mut u32) -> HRESULT {
        let mut index = self.index.lock().unwrap();
        unsafe {
            if cpins == 0 || *index >= 1 {
                if !pcfetched.is_null() {
                    *pcfetched = 0;
                }
                return S_FALSE;
            }
            *index += 1;
            if pppins.is_null() {
                return E_POINTER;
            }
            pppins.write(Some(self.pin.clone()));
            if !pcfetched.is_null() {
                *pcfetched = 1;
            }
        }
        if cpins == 1 {
            S_OK
        } else {
            S_FALSE
        }
    }
    fn Skip(&self, cpins: u32) -> WinResult<()> {
        if cpins > 0 {
            *self.index.lock().unwrap() = 1;
        }
        Ok(())
    }
    fn Reset(&self) -> WinResult<()> {
        *self.index.lock().unwrap() = 0;
        Ok(())
    }
    fn Clone(&self) -> WinResult<IEnumPins> {
        let index = *self.index.lock().unwrap();
        let e: IEnumPins = SinglePinEnum {
            pin: self.pin.clone(),
            index: Mutex::new(index),
        }
        .into();
        Ok(e)
    }
}

// ---------------------------------------------------------------------------
// IPin + IAMStreamConfig + IKsPropertySet — nosso único pino de saída.
// ---------------------------------------------------------------------------

#[implement(IPin, IAMStreamConfig, IKsPropertySet)]
struct VCamPin {
    inner: Arc<Inner>,
}

impl VCamPin_Impl {
    fn negotiate_allocator(&self, downstream: &IPin) -> WinResult<()> {
        unsafe {
            let mem_input: IMemInputPin = downstream.cast()?;
            let (_, _, fps) = current_video_params();
            let _ = fps;
            let (w, h, _) = current_video_params();
            let props = ALLOCATOR_PROPERTIES {
                cBuffers: 2,
                cbBuffer: rgb24_len(w, h) as i32,
                cbAlign: 1,
                cbPrefix: 0,
            };
            let allocator = mem_input.GetAllocator().or_else(|_| {
                windows::Win32::System::Com::CoCreateInstance::<Option<&IUnknown>, IMemAllocator>(
                    &windows::Win32::Media::MediaFoundation::CLSID_MemoryAllocator,
                    None,
                    windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                )
            })?;
            let _actual = allocator.SetProperties(&props)?;
            mem_input.NotifyAllocator(&allocator, false)?;
            allocator.Commit()?;

            *self.inner.mem_input.lock().unwrap() = Some(mem_input);
            *self.inner.allocator.lock().unwrap() = Some(allocator);
        }
        Ok(())
    }
}

impl IPin_Impl for VCamPin_Impl {
    fn Connect(&self, preceivepin: Ref<'_, IPin>, _pmt: *const AM_MEDIA_TYPE) -> WinResult<()> {
        let Some(downstream) = preceivepin.as_ref() else {
            return Err(E_POINTER.into());
        };
        let mt = build_media_type();
        let accept_hr = unsafe { downstream.QueryAccept(&mt) };
        if accept_hr.is_err() {
            free_media_type_format(&mt);
            return Err(windows::core::Error::from(accept_hr));
        }
        // pConnector é quem nós somos; vários downstreams exigem não-nulo e
        // retornam E_POINTER se receberem None (auto-referência guardada em
        // `self_pin`).
        let self_pin = self.inner.self_pin.lock().unwrap().clone();
        let recv = unsafe { downstream.ReceiveConnection(self_pin.as_ref(), &mt) };
        free_media_type_format(&mt);
        recv?;
        self.negotiate_allocator(downstream)?;
        *self.inner.peer.lock().unwrap() = Some(downstream.clone());
        Ok(())
    }

    fn ReceiveConnection(
        &self,
        _pconnector: Ref<'_, IPin>,
        _pmt: *const AM_MEDIA_TYPE,
    ) -> WinResult<()> {
        // Somos o pino de SAÍDA; quem inicia a conexão é o grafo, chamando
        // Connect() em nós — não o contrário.
        Err(E_UNEXPECTED.into())
    }

    fn Disconnect(&self) -> WinResult<()> {
        *self.inner.peer.lock().unwrap() = None;
        self.inner.mem_input.lock().unwrap().take();
        if let Some(alloc) = self.inner.allocator.lock().unwrap().take() {
            unsafe {
                let _ = alloc.Decommit();
            }
        }
        Ok(())
    }

    fn ConnectedTo(&self) -> WinResult<IPin> {
        self.inner
            .peer
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| windows::core::Error::from(E_NOINTERFACE))
    }

    fn ConnectionMediaType(&self, pmt: *mut AM_MEDIA_TYPE) -> WinResult<()> {
        if self.inner.peer.lock().unwrap().is_none() {
            return Err(E_UNEXPECTED.into());
        }
        unsafe {
            *pmt = build_media_type();
        }
        Ok(())
    }

    fn QueryPinInfo(&self, pinfo: *mut PIN_INFO) -> WinResult<()> {
        let filter = self.inner.self_filter.lock().unwrap().clone();
        unsafe {
            (*pinfo).pFilter = core::mem::ManuallyDrop::new(filter);
            (*pinfo).dir = PINDIR_OUTPUT;
            let name = "CamLink Video\0".encode_utf16().collect::<Vec<u16>>();
            let n = name.len().min((*pinfo).achName.len());
            (&mut (*pinfo).achName)[..n].copy_from_slice(&name[..n]);
        }
        Ok(())
    }

    fn QueryDirection(&self) -> WinResult<PIN_DIRECTION> {
        Ok(PINDIR_OUTPUT)
    }

    fn QueryId(&self) -> WinResult<windows::core::PWSTR> {
        unsafe {
            let id = "CamLinkVideoOut\0".encode_utf16().collect::<Vec<u16>>();
            let buf = CoTaskMemAlloc(id.len() * 2) as *mut u16;
            core::ptr::copy_nonoverlapping(id.as_ptr(), buf, id.len());
            Ok(windows::core::PWSTR(buf))
        }
    }

    fn QueryAccept(&self, pmt: *const AM_MEDIA_TYPE) -> HRESULT {
        unsafe {
            if pmt.is_null() || !media_type_matches(&*pmt) {
                return windows::Win32::Media::DirectShow::VFW_E_TYPE_NOT_ACCEPTED;
            }
        }
        S_OK
    }

    fn EnumMediaTypes(&self) -> WinResult<IEnumMediaTypes> {
        let e: IEnumMediaTypes = SingleMediaTypeEnum {
            done: Mutex::new(false),
        }
        .into();
        Ok(e)
    }

    fn QueryInternalConnections(
        &self,
        _appin: windows::core::OutRef<'_, IPin>,
        _npin: *mut u32,
    ) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }

    fn EndOfStream(&self) -> WinResult<()> {
        Err(E_UNEXPECTED.into())
    }
    fn BeginFlush(&self) -> WinResult<()> {
        Err(E_UNEXPECTED.into())
    }
    fn EndFlush(&self) -> WinResult<()> {
        Err(E_UNEXPECTED.into())
    }
    fn NewSegment(&self, _tstart: i64, _tstop: i64, _drate: f64) -> WinResult<()> {
        Err(E_UNEXPECTED.into())
    }
}

impl IAMStreamConfig_Impl for VCamPin_Impl {
    fn SetFormat(&self, pmt: *const AM_MEDIA_TYPE) -> WinResult<()> {
        unsafe {
            let ok = !pmt.is_null() && media_type_matches(&*pmt);
            if !ok {
                return Err(E_INVALIDARG.into());
            }
        }
        Ok(())
    }
    fn GetFormat(&self) -> WinResult<*mut AM_MEDIA_TYPE> {
        unsafe {
            let mt = build_media_type();
            let dst = CoTaskMemAlloc(size_of::<AM_MEDIA_TYPE>()) as *mut AM_MEDIA_TYPE;
            dst.write(mt);
            Ok(dst)
        }
    }
    fn GetNumberOfCapabilities(&self, picount: *mut i32, pisize: *mut i32) -> WinResult<()> {
        unsafe {
            *picount = 1;
            *pisize = size_of::<VIDEO_STREAM_CONFIG_CAPS>() as i32;
        }
        Ok(())
    }
    fn GetStreamCaps(
        &self,
        iindex: i32,
        ppmt: *mut *mut AM_MEDIA_TYPE,
        pscc: *mut u8,
    ) -> WinResult<()> {
        if iindex != 0 {
            return Err(E_INVALIDARG.into());
        }
        let (w, h, fps) = current_video_params();
        unsafe {
            let mt = build_media_type_for(w, h, fps);
            let dst = CoTaskMemAlloc(size_of::<AM_MEDIA_TYPE>()) as *mut AM_MEDIA_TYPE;
            dst.write(mt);
            *ppmt = dst;
            if !pscc.is_null() {
                let interval = 10_000_000 / i64::from(fps.max(1));
                let bps = (rgb24_len(w, h) as i32) * 8 * fps as i32;
                let size = SIZE {
                    cx: w as i32,
                    cy: h as i32,
                };
                let caps = VIDEO_STREAM_CONFIG_CAPS {
                    guid: FORMAT_VideoInfo,
                    VideoStandard: 0,
                    InputSize: size,
                    MinCroppingSize: size,
                    MaxCroppingSize: size,
                    CropGranularityX: 1,
                    CropGranularityY: 1,
                    CropAlignX: 0,
                    CropAlignY: 0,
                    MinOutputSize: size,
                    MaxOutputSize: size,
                    OutputGranularityX: 1,
                    OutputGranularityY: 1,
                    StretchTapsX: 0,
                    StretchTapsY: 0,
                    ShrinkTapsX: 0,
                    ShrinkTapsY: 0,
                    MinFrameInterval: interval,
                    MaxFrameInterval: interval,
                    MinBitsPerSecond: bps,
                    MaxBitsPerSecond: bps,
                };
                (pscc as *mut VIDEO_STREAM_CONFIG_CAPS).write(caps);
            }
        }
        Ok(())
    }
}

impl IKsPropertySet_Impl for VCamPin_Impl {
    fn Set(
        &self,
        _guidpropset: *const GUID,
        _dwpropid: u32,
        _pinstancedata: *const c_void,
        _cbinstancedata: u32,
        _ppropdata: *const c_void,
        _cbpropdata: u32,
    ) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }

    fn Get(
        &self,
        guidpropset: *const GUID,
        dwpropid: u32,
        _pinstancedata: *const c_void,
        _cbinstancedata: u32,
        ppropdata: *mut c_void,
        cbpropdata: u32,
        pcbreturned: *mut u32,
    ) -> WinResult<()> {
        unsafe {
            if guidpropset.is_null()
                || *guidpropset != AMPROPSETID_PIN
                || dwpropid != AMPROPERTY_PIN_CATEGORY
            {
                return Err(E_NOTIMPL.into());
            }
            if cbpropdata < size_of::<GUID>() as u32 {
                return Err(E_INVALIDARG.into());
            }
            (ppropdata as *mut GUID).write(PIN_CATEGORY_CAPTURE);
            if !pcbreturned.is_null() {
                *pcbreturned = size_of::<GUID>() as u32;
            }
        }
        Ok(())
    }

    fn QuerySupported(&self, guidpropset: *const GUID, dwpropid: u32) -> WinResult<u32> {
        unsafe {
            if !guidpropset.is_null()
                && *guidpropset == AMPROPSETID_PIN
                && dwpropid == AMPROPERTY_PIN_CATEGORY
            {
                Ok(1) // KSPROPERTY_SUPPORT_GET
            } else {
                Err(E_NOTIMPL.into())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// IBaseFilter — o filtro em si.
// ---------------------------------------------------------------------------

#[implement(IBaseFilter, IMediaFilter, IPersist)]
struct VCamFilter {
    inner: Arc<Inner>,
    pin: IPin,
}

impl VCamFilter {
    fn new() -> Self {
        let inner = Arc::new(Inner::new());
        let pin: IPin = VCamPin {
            inner: Arc::clone(&inner),
        }
        .into();
        *inner.self_pin.lock().unwrap() = Some(pin.clone());
        Self { inner, pin }
    }
}

impl IPersist_Impl for VCamFilter_Impl {
    fn GetClassID(&self) -> WinResult<GUID> {
        Ok(CLSID_CAMLINK_FILTER)
    }
}

impl IMediaFilter_Impl for VCamFilter_Impl {
    fn Stop(&self) -> WinResult<()> {
        self.inner.stop_running();
        Ok(())
    }
    fn Pause(&self) -> WinResult<()> {
        self.inner.ensure_producing(FILTER_STATE(1)); // State_Paused
        Ok(())
    }
    fn Run(&self, _tstart: i64) -> WinResult<()> {
        self.inner.ensure_producing(FILTER_STATE(2)); // State_Running
        Ok(())
    }
    fn GetState(&self, _dwmillisecstimeout: u32) -> WinResult<FILTER_STATE> {
        Ok(*self.inner.state.lock().unwrap())
    }
    fn SetSyncSource(&self, _pclock: Ref<'_, IReferenceClock>) -> WinResult<()> {
        Ok(())
    }
    fn GetSyncSource(&self) -> WinResult<IReferenceClock> {
        Err(E_NOTIMPL.into())
    }
}

impl IBaseFilter_Impl for VCamFilter_Impl {
    fn EnumPins(&self) -> WinResult<IEnumPins> {
        let e: IEnumPins = SinglePinEnum {
            pin: self.pin.clone(),
            index: Mutex::new(0),
        }
        .into();
        Ok(e)
    }
    fn FindPin(&self, _id: &PCWSTR) -> WinResult<IPin> {
        Ok(self.pin.clone())
    }
    fn QueryFilterInfo(&self, pinfo: *mut FILTER_INFO) -> WinResult<()> {
        // SAFETY: `ptr` só é não-None enquanto o grafo dono nos mantém vivos
        // (convenção DirectShow: filtro não faz AddRef do grafo);
        // reconstruímos uma referência emprestada a partir do ponteiro cru e
        // clonamos (AddRef de verdade) só a cópia que devolvemos.
        let graph = self
            .inner
            .graph
            .lock()
            .unwrap()
            .and_then(|ptr| unsafe { IFilterGraph::from_raw_borrowed(&ptr).cloned() });
        unsafe {
            (*pinfo).pGraph = core::mem::ManuallyDrop::new(graph);
            let name = format!("{FILTER_FRIENDLY_NAME}\0")
                .encode_utf16()
                .collect::<Vec<u16>>();
            let n = name.len().min((*pinfo).achName.len());
            (&mut (*pinfo).achName)[..n].copy_from_slice(&name[..n]);
        }
        Ok(())
    }
    fn JoinFilterGraph(&self, pgraph: Ref<'_, IFilterGraph>, _pname: &PCWSTR) -> WinResult<()> {
        *self.inner.graph.lock().unwrap() = pgraph.as_ref().map(|g| g.as_raw());
        Ok(())
    }
    fn QueryVendorInfo(&self) -> WinResult<windows::core::PWSTR> {
        Err(E_NOTIMPL.into())
    }
}

// ---------------------------------------------------------------------------
// IClassFactory + exports da DLL COM.
// ---------------------------------------------------------------------------

#[implement(IClassFactory)]
struct VCamClassFactory;

impl IClassFactory_Impl for VCamClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<'_, IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> WinResult<()> {
        if punkouter.as_ref().is_some() {
            return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into());
        }
        let vcam = VCamFilter::new();
        let inner = Arc::clone(&vcam.inner);
        let filter: IBaseFilter = vcam.into();
        *inner.self_filter.lock().unwrap() = Some(filter.clone());
        unsafe { filter.query(riid, ppvobject).ok() }
    }
    fn LockServer(&self, _flock: BOOL) -> WinResult<()> {
        Ok(())
    }
}

/// Ponto de entrada COM padrão (`regsvr32`/`CoCreateInstance`) — nunca
/// chamado diretamente pelo nosso código, só pelo runtime COM do Windows.
///
/// # Safety
/// Chamado pelo runtime COM com ponteiros que ele garante válidos por
/// contrato da ABI `DllGetClassObject`; nós mesmos validamos null antes de
/// desreferenciar `rclsid`.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    if *rclsid != CLSID_CAMLINK_FILTER {
        return windows::Win32::Foundation::CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = VCamClassFactory.into();
    factory.query(riid, ppv)
}

/// # Safety
/// Assinatura fixa pela ABI COM (`DllCanUnloadNow`); não recebe ponteiros,
/// nenhuma invariante adicional além de rodar após a DLL estar carregada.
#[no_mangle]
pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

// ---------------------------------------------------------------------------
// Registro: escreve em HKEY_CURRENT_USER\Software\Classes — sem elevação (o
// Windows mescla essa hive sobre HKEY_CLASSES_ROOT para o mesmo usuário na
// hora da leitura, então Chrome/OBS enxergam o filtro normalmente).
// ---------------------------------------------------------------------------

fn guid_to_reg_string(g: &GUID) -> String {
    format!(
        "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}

unsafe fn reg_set_sz(
    parent: HKEY,
    subpath: &str,
    value_name: Option<&str>,
    data: &str,
) -> WIN32_ERROR {
    let mut hkey = HKEY::default();
    let path = wide(subpath);
    let err = RegCreateKeyExW(
        parent,
        PCWSTR(path.as_ptr()),
        None,
        PCWSTR::null(),
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE,
        None,
        &mut hkey,
        None,
    );
    if err != WIN32_ERROR(0) {
        return err;
    }
    let data_w = wide(data);
    let bytes = core::slice::from_raw_parts(data_w.as_ptr() as *const u8, data_w.len() * 2);
    let name = value_name.map(wide);
    let name_ptr = match &name {
        Some(n) => PCWSTR(n.as_ptr()),
        None => PCWSTR::null(),
    };
    let err = RegSetValueExW(hkey, name_ptr, None, REG_SZ, Some(bytes));
    let _ = RegCloseKey(hkey);
    err
}

fn dll_path() -> WinResult<String> {
    unsafe {
        // Handle DESTE módulo (a DLL), não do processo host — via
        // GetModuleHandleExW com o endereço desta própria função.
        let mut this_module = HMODULE::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            PCWSTR(DllGetClassObject as *const u16),
            &mut this_module,
        )?;
        let mut buf = vec![0u16; 1024];
        let len = GetModuleFileNameW(Some(this_module), &mut buf);
        if len == 0 {
            return Err(windows::core::Error::from(HRESULT::from_win32(
                GetLastError().0,
            )));
        }
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// # Safety
/// Assinatura fixa pela ABI COM (`DllRegisterServer`, chamada por
/// `regsvr32`/o instalador); não recebe ponteiros do chamador.
#[no_mangle]
pub unsafe extern "system" fn DllRegisterServer() -> HRESULT {
    let path = match dll_path() {
        Ok(p) => p,
        Err(e) => return e.code(),
    };
    let clsid_str = guid_to_reg_string(&CLSID_CAMLINK_FILTER);
    let category_str = guid_to_reg_string(&CLSID_VIDEO_INPUT_DEVICE_CATEGORY);

    let mut err = reg_set_sz(
        HKEY_CURRENT_USER,
        &format!("Software\\Classes\\CLSID\\{clsid_str}"),
        None,
        FILTER_FRIENDLY_NAME,
    );
    if err == WIN32_ERROR(0) {
        err = reg_set_sz(
            HKEY_CURRENT_USER,
            &format!("Software\\Classes\\CLSID\\{clsid_str}\\InprocServer32"),
            None,
            &path,
        );
    }
    if err == WIN32_ERROR(0) {
        err = reg_set_sz(
            HKEY_CURRENT_USER,
            &format!("Software\\Classes\\CLSID\\{clsid_str}\\InprocServer32"),
            Some("ThreadingModel"),
            "Both",
        );
    }
    if err == WIN32_ERROR(0) {
        err = reg_set_sz(
            HKEY_CURRENT_USER,
            &format!("Software\\Classes\\CLSID\\{category_str}\\Instance\\{clsid_str}"),
            Some("CLSID"),
            &clsid_str,
        );
    }
    if err == WIN32_ERROR(0) {
        err = reg_set_sz(
            HKEY_CURRENT_USER,
            &format!("Software\\Classes\\CLSID\\{category_str}\\Instance\\{clsid_str}"),
            Some("FriendlyName"),
            FILTER_FRIENDLY_NAME,
        );
    }
    if err == WIN32_ERROR(0) {
        S_OK
    } else {
        HRESULT::from_win32(err.0)
    }
}

/// # Safety
/// Assinatura fixa pela ABI COM (`DllUnregisterServer`, chamada por
/// `regsvr32`/o instalador); não recebe ponteiros do chamador.
#[no_mangle]
pub unsafe extern "system" fn DllUnregisterServer() -> HRESULT {
    use windows::Win32::System::Registry::RegDeleteTreeW;
    let clsid_str = guid_to_reg_string(&CLSID_CAMLINK_FILTER);
    let category_str = guid_to_reg_string(&CLSID_VIDEO_INPUT_DEVICE_CATEGORY);

    let mut hkey = HKEY::default();
    let base = wide("Software\\Classes");
    if RegCreateKeyExW(
        HKEY_CURRENT_USER,
        PCWSTR(base.as_ptr()),
        None,
        PCWSTR::null(),
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE,
        None,
        &mut hkey,
        None,
    ) == WIN32_ERROR(0)
    {
        let clsid_key = wide(&format!("CLSID\\{clsid_str}"));
        let _ = RegDeleteTreeW(hkey, PCWSTR(clsid_key.as_ptr()));
        let inst_key = wide(&format!("CLSID\\{category_str}\\Instance\\{clsid_str}"));
        let _ = RegDeleteTreeW(hkey, PCWSTR(inst_key.as_ptr()));
        let _ = RegCloseKey(hkey);
    }
    S_OK
}

// ---------------------------------------------------------------------------
// DShowBackend — lado produtor (`VirtualCameraBackend`), roda no processo
// do CamLink. Nunca instancia `VCamFilter` diretamente: só escreve na
// memória compartilhada que a worker thread do lado consumidor lê.
// ---------------------------------------------------------------------------

struct ManagedCamera {
    camera: VirtualCamera,
    hub: FrameHub,
    resolution: (u32, u32),
    fps: u32,
}

/// Backend real de câmera virtual via o filtro DirectShow próprio. v1
/// suporta uma única câmera por vez (ver limitação documentada no topo do
/// arquivo) — `create()` retorna erro se já houver uma ativa.
pub struct DShowBackend {
    camera: Option<ManagedCamera>,
}

impl Default for DShowBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DShowBackend {
    pub fn new() -> Self {
        Self { camera: None }
    }
}

impl VirtualCameraBackend for DShowBackend {
    fn create(
        &mut self,
        label: &str,
        resolution: (u32, u32),
        fps: u32,
    ) -> Result<VirtualCamera, VcamError> {
        if self.camera.is_some() {
            return Err(VcamError::Backend(
                "já existe uma câmera virtual DirectShow ativa (limite v1: uma por vez)".into(),
            ));
        }
        if resolution.0 > FRAME_MAX_WIDTH || resolution.1 > FRAME_MAX_HEIGHT {
            return Err(VcamError::Backend(format!(
                "resolução {}x{} excede o máximo suportado ({FRAME_MAX_WIDTH}x{FRAME_MAX_HEIGHT})",
                resolution.0, resolution.1
            )));
        }
        let hub = FrameHub::connect().map_err(|e| {
            VcamError::Backend(format!("falha ao abrir memória compartilhada: {e}"))
        })?;

        let camera = VirtualCamera {
            id: Uuid::new_v4(),
            label: label.to_string(),
            backend_path: guid_to_reg_string(&CLSID_CAMLINK_FILTER),
            state: VirtualCameraState::Standby,
        };

        let standby = standby_frame(resolution, "Aguardando fonte...");
        let mut rgb24 = standby;
        convert_rgba_to_rgb24_in_place(&mut rgb24);
        hub.write_frame(resolution.0, resolution.1, fps, &rgb24, false);

        tracing::info!(id = %camera.id, %label, "câmera virtual DirectShow criada");
        self.camera = Some(ManagedCamera {
            camera: camera.clone(),
            hub,
            resolution,
            fps,
        });
        Ok(camera)
    }

    fn feed_frame(&mut self, id: &Uuid, frame: &[u8]) -> Result<(), VcamError> {
        if frame.is_empty() {
            return Err(VcamError::InvalidFrame("frame vazio".into()));
        }
        let managed = self
            .camera
            .as_mut()
            .filter(|m| &m.camera.id == id)
            .ok_or(VcamError::NotFound(*id))?;
        let expected_rgba = managed.resolution.0 as usize * managed.resolution.1 as usize * 4;
        if frame.len() != expected_rgba {
            return Err(VcamError::InvalidFrame(format!(
                "tamanho do frame ({}) não bate com a resolução declarada \
                 ({expected_rgba} bytes esperados, RGBA)",
                frame.len()
            )));
        }
        let mut rgb24 = frame.to_vec();
        convert_rgba_to_rgb24_in_place(&mut rgb24);
        managed.hub.write_frame(
            managed.resolution.0,
            managed.resolution.1,
            managed.fps,
            &rgb24,
            true,
        );
        managed.camera.state = VirtualCameraState::Live;
        Ok(())
    }

    fn set_standby(&mut self, id: &Uuid, message: &str) -> Result<(), VcamError> {
        let managed = self
            .camera
            .as_mut()
            .filter(|m| &m.camera.id == id)
            .ok_or(VcamError::NotFound(*id))?;
        let mut rgb24 = standby_frame(managed.resolution, message);
        convert_rgba_to_rgb24_in_place(&mut rgb24);
        managed.hub.write_frame(
            managed.resolution.0,
            managed.resolution.1,
            managed.fps,
            &rgb24,
            false,
        );
        managed.camera.state = VirtualCameraState::Standby;
        Ok(())
    }

    fn destroy(&mut self, id: &Uuid) -> Result<(), VcamError> {
        match &self.camera {
            Some(m) if &m.camera.id == id => {
                self.camera = None;
                Ok(())
            }
            _ => Err(VcamError::NotFound(*id)),
        }
    }

    fn camera(&self, id: &Uuid) -> Option<&VirtualCamera> {
        self.camera
            .as_ref()
            .filter(|m| &m.camera.id == id)
            .map(|m| &m.camera)
    }

    fn cameras(&self) -> Vec<&VirtualCamera> {
        self.camera.iter().map(|m| &m.camera).collect()
    }
}
