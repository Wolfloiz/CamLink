//! Spike C (T074) — filtro DirectShow push-source próprio.
//!
//! Substitui o akvirtualcamera (Spike B, T016, reprovado — ver
//! `specs/001-phone-webcam-bridge/research.md` R4) por um filtro DirectShow
//! implementado do zero em Rust via `windows-rs`, sem depender de driver de
//! terceiros. Arquitetura de referência: amostra oficial `PushSource` da
//! Microsoft (MIT,
//! `Samples/Win7Samples/multimedia/directshow/filters/pushsource/` em
//! `microsoft/Windows-classic-samples`) — mesma categoria de filtro (fonte
//! "push": o próprio filtro gera/envia frames, ao contrário de uma fonte
//! "live" que é puxada pelo grafo).
//!
//! Este binário É a câmera virtual: compila para uma DLL COM
//! (`crate-type = ["cdylib"]`) que se registra como fonte de vídeo
//! DirectShow. O worker thread do filtro gera sozinho o mesmo padrão de
//! teste do Spike B (barra de timestamp + barras de cor) assim que o grafo
//! chama `Run()` — não há processo externo empurrando frames nesta spike
//! (isso fica para o T023, que vai canalizar frames reais do pipeline
//! decode→push).
//!
//! Registro: usa `HKEY_CURRENT_USER\Software\Classes` (sem elevação —
//! Windows mescla essa hive sobre `HKEY_CLASSES_ROOT` para o mesmo usuário
//! que registrou, então consumidores como Chrome/OBS enxergam o filtro sem
//! precisar rodar como admin).
//!
//! Uso:
//! ```text
//! cargo build --example win_dshow_spike --release
//! regsvr32 /s target\release\examples\win_dshow_spike.dll
//! :: abrir OBS/Chrome/Firefox/Discord e selecionar "CamLink Spike C"
//! regsvr32 /u /s target\release\examples\win_dshow_spike.dll
//! ```

#[cfg(target_os = "windows")]
extern crate windows_core;

#[cfg(not(target_os = "windows"))]
// win_dshow_spike é específico do Windows (Spike C / T074). Em outras
// plataformas este crate-type cdylib compila vazio (sem exports).
const _: () = ();

#[cfg(target_os = "windows")]
mod dshow_filter {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use windows::core::{
        implement, IUnknown, Interface, Ref, Result as WinResult, BOOL, GUID, HRESULT, PCWSTR,
    };

    // -------------------------------------------------------------------
    // Log de diagnóstico em arquivo — a OBS não tem console visível, então
    // é a única forma de ver a sequência real de chamadas até o travamento.
    // Remover antes de consolidar T023.
    // -------------------------------------------------------------------
    static DBG_LOG_LOCK: Mutex<()> = Mutex::new(());

    fn dbg_log(msg: &str) {
        use std::io::Write;
        // Sem essa trava, duas threads escrevendo quase simultaneamente
        // produzem linhas intercaladas byte-a-byte (viu-se na prática:
        // trechos de "Filter::Run" e "QueryFilterInfo" emendados numa
        // única linha corrompida) — inofensivo pro filtro em si, mas
        // atrapalha diagnosticar a ordem real dos eventos.
        let _guard = DBG_LOG_LOCK.lock().unwrap();
        let path = std::env::temp_dir().join("camlink_dshow_debug.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(
                f,
                "[{:?}] [tid={:?}] {}",
                std::time::Instant::now(),
                std::thread::current().id(),
                msg
            );
        }
    }
    use windows::Win32::Foundation::{
        GetLastError, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER, E_UNEXPECTED, HMODULE,
        S_FALSE, S_OK,
    };
    use windows::Win32::Foundation::{SIZE, WIN32_ERROR};
    use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;
    use windows::Win32::Media::DirectShow::{
        IAMStreamConfig, IAMStreamConfig_Impl, IBaseFilter, IBaseFilter_Impl, IEnumMediaTypes,
        IEnumMediaTypes_Impl, IEnumPins, IEnumPins_Impl, IFilterGraph, IMediaFilter,
        IMediaFilter_Impl, IMemAllocator, IMemInputPin, IPin, IPin_Impl, ALLOCATOR_PROPERTIES,
        FILTER_INFO, FILTER_STATE, PINDIR_OUTPUT, PIN_DIRECTION, PIN_INFO,
        VIDEO_STREAM_CONFIG_CAPS,
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
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    // -------------------------------------------------------------------
    // Identidade do filtro
    // -------------------------------------------------------------------

    const CLSID_CAMLINK_FILTER: GUID = GUID::from_u128(0xe7a04118_a6bc_496d_9325_c0e2d265a006);
    const FILTER_FRIENDLY_NAME: &str = "CamLink Spike C";

    /// `CLSID_VideoInputDeviceCategory` (uuids.h) — categoria em que
    /// consumidores DirectShow (`ICreateDevEnum`) procuram fontes de vídeo.
    const CLSID_VIDEO_INPUT_DEVICE_CATEGORY: GUID =
        GUID::from_u128(0x860bb310_5d01_11d0_bd3b_00a0c911ce86);

    /// `PIN_CATEGORY_CAPTURE` (ksmedia.h) — identifica o pino como a saída
    /// de captura principal (vs. preview) para quem consulta via
    /// `IKsPropertySet`.
    const PIN_CATEGORY_CAPTURE: GUID = GUID::from_u128(0xfb6c4281_0353_11d1_905f_0000c0cc16ba);
    /// `AMPROPSETID_Pin` (amvideo.h / ksmedia.h).
    const AMPROPSETID_PIN: GUID = GUID::from_u128(0x9b00f101_1567_11d1_b3f1_00aa003761c5);
    const AMPROPERTY_PIN_CATEGORY: u32 = 0;

    const WIDTH: i32 = 640;
    const HEIGHT: i32 = 480;
    const FPS: i32 = 30;
    const FRAME_SIZE: usize = (WIDTH * HEIGHT * 3) as usize;
    const BARCODE_BITS: u32 = 24;
    const BARCODE_WIDTH: i32 = 240;

    // -------------------------------------------------------------------
    // Geração do frame de teste (mesmo padrão do Spike B: barra de
    // timestamp em bits + barras de cor, para checagem visual).
    // -------------------------------------------------------------------

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
                if px + 2 >= buf.len() {
                    continue;
                }
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

    // -------------------------------------------------------------------
    // AM_MEDIA_TYPE: único formato suportado (RGB24 640x480@30).
    // `pbFormat` é alocado via CoTaskMemAlloc — dono é quem recebe o
    // AM_MEDIA_TYPE (convenção COM padrão; o chamador libera com
    // DeleteMediaType/CoTaskMemFree).
    // -------------------------------------------------------------------

    fn build_media_type() -> AM_MEDIA_TYPE {
        unsafe {
            let cb = size_of::<VIDEOINFOHEADER>();
            let pb = CoTaskMemAlloc(cb) as *mut u8;
            assert!(!pb.is_null(), "CoTaskMemAlloc falhou");
            let vih = pb as *mut VIDEOINFOHEADER;
            core::ptr::write_bytes(vih, 0, 1);
            (*vih).AvgTimePerFrame = 10_000_000 / FPS as i64;
            (*vih).bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            (*vih).bmiHeader.biWidth = WIDTH;
            (*vih).bmiHeader.biHeight = HEIGHT; // positivo = bottom-up (DIB padrão)
            (*vih).bmiHeader.biPlanes = 1;
            (*vih).bmiHeader.biBitCount = 24;
            (*vih).bmiHeader.biCompression = 0; // BI_RGB
            (*vih).bmiHeader.biSizeImage = FRAME_SIZE as u32;

            AM_MEDIA_TYPE {
                majortype: MEDIATYPE_Video,
                subtype: MEDIASUBTYPE_RGB24,
                bFixedSizeSamples: BOOL::from(true),
                bTemporalCompression: BOOL::from(false),
                lSampleSize: FRAME_SIZE as u32,
                formattype: FORMAT_VideoInfo,
                pUnk: core::mem::ManuallyDrop::new(None),
                cbFormat: cb as u32,
                pbFormat: pb,
            }
        }
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

    // -------------------------------------------------------------------
    // Estado compartilhado entre o filtro e seu único pino de saída.
    // -------------------------------------------------------------------

    struct Inner {
        state: Mutex<FILTER_STATE>,
        peer: Mutex<Option<IPin>>,
        /// `IMemInputPin`/`IMemAllocator` do downstream. Guardados como
        /// ponteiro cru mesmo, compartilhado com a worker thread — NÃO via
        /// Global Interface Table. `IMemInputPin`/`IMemAllocator` não têm
        /// proxy/stub COM registrado no sistema (são interfaces "fast path"
        /// do DirectShow, propositalmente não-marshaláveis:
        /// `GetInterfaceFromGlobal` falha com E_UNEXPECTED/"proxy não
        /// encontrado" pra elas). O próprio código da OBS
        /// (`win-dshow.cpp`) documenta a convenção real: "Always keep
        /// directshow in a single thread for a given device" — ou seja, o
        /// modelo esperado é ponteiro cru cross-thread mesmo, com a
        /// responsabilidade de thread-safety do LADO DE QUEM IMPLEMENTA
        /// `Receive`, não do COM. Todo hardware de captura real funciona
        /// assim.
        mem_input: Mutex<Option<IMemInputPin>>,
        allocator: Mutex<Option<IMemAllocator>>,
        running: Arc<AtomicBool>,
        worker: Mutex<Option<JoinHandle<()>>>,
        start: Instant,
        /// Referência contada (AddRef'd) do filtro dono, exigida por
        /// `IPin::QueryPinInfo::pFilter` — todo pino DEVE ter um filtro
        /// dono não-nulo; chamadores (ex.: Filter Graph Manager) desreferenciam
        /// sem checar null, então deixar `None` aqui derruba o processo
        /// hospedeiro. Preenchida por `VCamClassFactory::CreateInstance`
        /// logo após a conversão para `IBaseFilter`.
        self_filter: Mutex<Option<IBaseFilter>>,
        /// Ponteiro NÃO contado (sem AddRef, por convenção DirectShow — evita
        /// ciclo grafo→filtro→grafo) para o `IFilterGraph` ao qual fomos
        /// unidos via `JoinFilterGraph`. `QueryFilterInfo` precisa devolver
        /// isso via referência contada nova (AddRef só na hora da consulta);
        /// deixar sempre `None` faz o Filter Graph Manager concluir que o
        /// filtro não está no grafo (`VFW_E_NOT_IN_GRAPH`) e falhar o Connect.
        graph: Mutex<Option<*mut c_void>>,
        /// Referência contada a nós mesmos como `IPin` — vários pinos
        /// downstream exigem um `pConnector` não-nulo em `ReceiveConnection`
        /// (retornam `E_POINTER` se receberem `None`), então `Connect`
        /// precisa se identificar de verdade, não passar `None`.
        self_pin: Mutex<Option<IPin>>,
    }

    // SAFETY: os ponteiros COM guardados aqui (IPin/IMemInputPin/IMemAllocator)
    // só são acessados sob o Mutex correspondente, nunca concorrentemente sem
    // sincronização — o filtro se registra com ThreadingModel=Both, ou seja,
    // é responsabilidade nossa (não do COM) serializar o acesso entre a
    // thread do grafo e a worker thread, o que os Mutex já garantem.
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
                start: Instant::now(),
                self_filter: Mutex::new(None),
                graph: Mutex::new(None),
                self_pin: Mutex::new(None),
            }
        }

        /// Fontes "ao vivo" (não-arquivo) devem começar a produzir samples já
        /// em `Paused`, não só em `Running`: o Filter Graph Manager espera
        /// conseguir puxar a primeira amostra durante a transição
        /// Stopped→Paused para considerar a transição concluída. Um source
        /// que só produz em `Run()` deixa consumidores (ex.: preview da OBS)
        /// travados nesse preroll, mostrando tela preta indefinidamente.
        fn ensure_producing(self: &Arc<Self>, state: FILTER_STATE) {
            dbg_log(&format!("ensure_producing: state={state:?} begin"));
            *self.state.lock().unwrap() = state;
            if self.running.swap(true, Ordering::SeqCst) {
                dbg_log("ensure_producing: já rodando, retornando");
                return; // já rodando
            }
            let inner = Arc::clone(self);
            let handle = thread::spawn(move || worker_loop(inner));
            *self.worker.lock().unwrap() = Some(handle);
            dbg_log("ensure_producing: thread spawnada, retornando");
        }

        fn stop_running(self: &Arc<Self>) {
            dbg_log("stop_running: begin");
            *self.state.lock().unwrap() = FILTER_STATE(0); // State_Stopped
            self.running.store(false, Ordering::SeqCst);
            // NÃO fazemos join() aqui: Stop() roda na thread do chamador
            // (ex.: a UI/STA da OBS), e a worker thread pode estar parada
            // dentro de uma chamada COM marshalada (via GIT) que só é
            // entregue se essa MESMA thread continuar bombeando mensagens.
            // Um join() bloqueante aqui é exatamente o "cada um espera o
            // outro" que travava o encerramento da OBS: a thread do
            // chamador parada no join(), a worker parada esperando essa
            // mesma thread processar a chamada marshalada. Deixamos a
            // worker thread perceber `running=false` e sair sozinha; o
            // `JoinHandle` órfão só é dropado (a thread do SO segue até
            // retornar por conta própria, sem bloquear ninguém).
            self.worker.lock().unwrap().take();
        }
    }

    fn worker_loop(inner: Arc<Inner>) {
        dbg_log("worker_loop: start");
        // SAFETY: toda thread que participa de COM precisa se inicializar
        // antes de fazer qualquer chamada COM, mesmo chamadas "cruas" sem
        // marshaling como as que fazemos aqui (ver nota em `Inner` sobre
        // `mem_input`/`allocator`).
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            dbg_log(&format!("worker_loop: CoInitializeEx -> {hr:?}"));
        }

        let mem_input = inner.mem_input.lock().unwrap().clone();
        let allocator = inner.allocator.lock().unwrap().clone();
        dbg_log(&format!(
            "worker_loop: mem_input={} allocator={}",
            mem_input.is_some(),
            allocator.is_some()
        ));

        if let (Some(mem_input), Some(allocator)) = (mem_input, allocator) {
            dbg_log("worker_loop: entrando no loop de frames");
            let frame_period = Duration::from_secs_f64(1.0 / FPS as f64);
            // REFERENCE_TIME (unidades de 100ns) por frame, casado com o
            // AvgTimePerFrame que declaramos no media type.
            let frame_duration_rt: i64 = 10_000_000 / FPS as i64;
            let mut frame = vec![0u8; FRAME_SIZE];
            let mut iter: u64 = 0;
            while inner.running.load(Ordering::SeqCst) {
                let loop_start = Instant::now();
                unsafe {
                    let mut sample = None;
                    dbg_log(&format!("worker_loop[{iter}]: chamando allocator.GetBuffer..."));
                    let hr = allocator.GetBuffer(&mut sample, None, None, 0);
                    dbg_log(&format!("worker_loop[{iter}]: GetBuffer -> {hr:?}"));
                    if hr.is_ok() {
                        if let Some(sample) = sample {
                            if let Ok(ptr) = sample.GetPointer() {
                                let len = (sample.GetSize() as usize).min(FRAME_SIZE);
                                make_frame(inner.start.elapsed().as_millis() as u32, &mut frame);
                                core::ptr::copy_nonoverlapping(frame.as_ptr(), ptr, len);
                            }
                            let _ = sample.SetActualDataLength(FRAME_SIZE as i32);
                            let _ = sample.SetSyncPoint(true);
                            // Sem timestamp, `IMediaSample::GetTime` falha e
                            // consumidores que só encaminham o frame pro
                            // pipeline de vídeo real quando têm um tempo
                            // válido (ex.: `HDevice::Receive` da OBS,
                            // `libdshowcapture`) descartam a amostra
                            // silenciosamente — o `Receive()` retorna S_OK
                            // (aceita no nível do protocolo) mas o conteúdo
                            // nunca chega a ser renderizado. Foi exatamente
                            // o sintoma visto: tela preta / preview travado
                            // no último frame de outra fonte.
                            let start_rt = iter as i64 * frame_duration_rt;
                            let end_rt = start_rt + frame_duration_rt;
                            let _ = sample.SetTime(Some(&start_rt), Some(&end_rt));
                            dbg_log(&format!("worker_loop[{iter}]: chamando mem_input.Receive..."));
                            let recv_hr = mem_input.Receive(&sample);
                            dbg_log(&format!("worker_loop[{iter}]: Receive -> {recv_hr:?}"));
                        }
                    }
                }
                dbg_log(&format!("worker_loop[{iter}]: iteração completa"));
                iter += 1;
                let spent = loop_start.elapsed();
                if spent < frame_period {
                    thread::sleep(frame_period - spent);
                }
            }
        }
        dbg_log("worker_loop: saindo (running=false ou faltou mem_input/allocator)");

        // SAFETY: contrabalança o CoInitializeEx no início desta thread.
        unsafe {
            CoUninitialize();
        }
    }

    // -------------------------------------------------------------------
    // IEnumMediaTypes — só temos um formato.
    // -------------------------------------------------------------------

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
                dbg_log(&format!(
                    "SingleMediaTypeEnum::Next cmediatypes={cmediatypes} done={}",
                    *done
                ));
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
                // Convenção COM: S_OK só quando fetched == celt pedido;
                // como só temos 1 formato, qualquer cmediatypes > 1 é
                // busca parcial (S_FALSE), mesmo tendo retornado 1 item.
                if cmediatypes == 1 {
                    S_OK
                } else {
                    S_FALSE
                }
            }
        }
        fn Skip(&self, cmediatypes: u32) -> WinResult<()> {
            dbg_log(&format!("SingleMediaTypeEnum::Skip cmediatypes={cmediatypes}"));
            if cmediatypes == 0 {
                return Ok(());
            }
            // Contrato COM: Skip precisa REALMENTE consumir posições, senão
            // um chamador que faz Reset+Skip(i)+Next em busca do fim da
            // lista (padrão comum pra "sondar" quantos itens existem) nunca
            // vê Next falhar e sonda i=0,1,2,... para sempre. Como só temos
            // 1 item, qualquer Skip(>=1) já esgota o enumerador.
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
            dbg_log("SingleMediaTypeEnum::Reset");
            *self.done.lock().unwrap() = false;
            Ok(())
        }
        fn Clone(&self) -> WinResult<IEnumMediaTypes> {
            dbg_log("SingleMediaTypeEnum::Clone");
            let done = *self.done.lock().unwrap();
            let e: IEnumMediaTypes = SingleMediaTypeEnum {
                done: Mutex::new(done),
            }
            .into();
            Ok(e)
        }
    }

    // -------------------------------------------------------------------
    // IEnumPins — só temos um pino.
    // -------------------------------------------------------------------

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
            // Convenção COM: S_OK só quando fetched == cpins pedido.
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

    // -------------------------------------------------------------------
    // IPin + IAMStreamConfig + IKsPropertySet — nosso único pino de saída.
    // -------------------------------------------------------------------

    #[implement(IPin, IAMStreamConfig, IKsPropertySet)]
    struct VCamPin {
        inner: Arc<Inner>,
    }

    impl VCamPin_Impl {
        fn negotiate_allocator(&self, downstream: &IPin) -> WinResult<()> {
            dbg_log("negotiate_allocator: begin");
            unsafe {
                let mem_input: IMemInputPin = downstream.cast()?;
                dbg_log("negotiate_allocator: cast IMemInputPin OK");
                let props = ALLOCATOR_PROPERTIES {
                    cBuffers: 2,
                    cbBuffer: FRAME_SIZE as i32,
                    cbAlign: 1,
                    cbPrefix: 0,
                };
                let allocator =
                    mem_input.GetAllocator().or_else(|_| {
                        windows::Win32::System::Com::CoCreateInstance::<
                            Option<&IUnknown>,
                            IMemAllocator,
                        >(
                            &windows::Win32::Media::MediaFoundation::CLSID_MemoryAllocator,
                            None,
                            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                        )
                    })?;
                dbg_log("negotiate_allocator: obteve allocator");
                let _actual = allocator.SetProperties(&props)?;
                dbg_log("negotiate_allocator: SetProperties OK");
                mem_input.NotifyAllocator(&allocator, false)?;
                dbg_log("negotiate_allocator: NotifyAllocator OK");
                allocator.Commit()?;
                dbg_log("negotiate_allocator: Commit OK");

                *self.inner.mem_input.lock().unwrap() = Some(mem_input);
                *self.inner.allocator.lock().unwrap() = Some(allocator);
            }
            dbg_log("negotiate_allocator: end OK");
            Ok(())
        }
    }

    impl IPin_Impl for VCamPin_Impl {
        fn Connect(&self, preceivepin: Ref<'_, IPin>, _pmt: *const AM_MEDIA_TYPE) -> WinResult<()> {
            dbg_log("Connect: begin");
            let Some(downstream) = preceivepin.as_ref() else {
                dbg_log("Connect: preceivepin nulo");
                return Err(E_POINTER.into());
            };
            let mt = build_media_type();
            dbg_log("Connect: chamando downstream.QueryAccept...");
            let accept_hr = unsafe { downstream.QueryAccept(&mt) };
            dbg_log(&format!("Connect: QueryAccept -> {accept_hr:?}"));
            if accept_hr.is_err() {
                free_media_type_format(&mt);
                return Err(windows::core::Error::from(accept_hr));
            }
            // pConnector é quem nós somos; vários downstreams (ex.: o Null
            // Renderer) exigem não-nulo e retornam E_POINTER se receberem
            // None, então usamos a auto-referência guardada em `self_pin`.
            let self_pin = self.inner.self_pin.lock().unwrap().clone();
            dbg_log("Connect: chamando downstream.ReceiveConnection...");
            let recv = unsafe { downstream.ReceiveConnection(self_pin.as_ref(), &mt) };
            dbg_log(&format!("Connect: ReceiveConnection -> {recv:?}"));
            free_media_type_format(&mt);
            recv?;
            dbg_log("Connect: chamando negotiate_allocator...");
            self.negotiate_allocator(downstream)?;
            *self.inner.peer.lock().unwrap() = Some(downstream.clone());
            dbg_log("Connect: end OK");
            Ok(())
        }

        fn ReceiveConnection(
            &self,
            _pconnector: Ref<'_, IPin>,
            _pmt: *const AM_MEDIA_TYPE,
        ) -> WinResult<()> {
            dbg_log("ReceiveConnection (nosso, deveria ser raro/nunca chamado)");
            // Somos o pino de SAÍDA; quem inicia a conexão é o grafo,
            // chamando Connect() em nós — não o contrário.
            Err(E_UNEXPECTED.into())
        }

        fn Disconnect(&self) -> WinResult<()> {
            dbg_log("Disconnect");
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
            dbg_log("QueryAccept (nosso pino sendo consultado)");
            unsafe {
                if pmt.is_null() || !media_type_matches(&*pmt) {
                    return windows::Win32::Media::DirectShow::VFW_E_TYPE_NOT_ACCEPTED;
                }
            }
            S_OK
        }

        fn EnumMediaTypes(&self) -> WinResult<IEnumMediaTypes> {
            dbg_log("EnumMediaTypes");
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
                dbg_log(&format!("SetFormat: matches={ok}"));
                if !ok {
                    return Err(E_INVALIDARG.into());
                }
            }
            Ok(())
        }
        fn GetFormat(&self) -> WinResult<*mut AM_MEDIA_TYPE> {
            dbg_log("GetFormat");
            unsafe {
                let mt = build_media_type();
                let dst = CoTaskMemAlloc(size_of::<AM_MEDIA_TYPE>()) as *mut AM_MEDIA_TYPE;
                dst.write(mt);
                Ok(dst)
            }
        }
        fn GetNumberOfCapabilities(&self, picount: *mut i32, pisize: *mut i32) -> WinResult<()> {
            dbg_log("GetNumberOfCapabilities");
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
            dbg_log(&format!("GetStreamCaps: iindex={iindex}"));
            if iindex != 0 {
                return Err(E_INVALIDARG.into());
            }
            unsafe {
                let mt = build_media_type();
                let dst = CoTaskMemAlloc(size_of::<AM_MEDIA_TYPE>()) as *mut AM_MEDIA_TYPE;
                dst.write(mt);
                *ppmt = dst;
                if !pscc.is_null() {
                    let interval = 10_000_000 / FPS as i64;
                    let bps = (FRAME_SIZE as i32) * 8 * FPS;
                    let caps = VIDEO_STREAM_CONFIG_CAPS {
                        guid: FORMAT_VideoInfo,
                        VideoStandard: 0,
                        InputSize: SIZE {
                            cx: WIDTH,
                            cy: HEIGHT,
                        },
                        MinCroppingSize: SIZE {
                            cx: WIDTH,
                            cy: HEIGHT,
                        },
                        MaxCroppingSize: SIZE {
                            cx: WIDTH,
                            cy: HEIGHT,
                        },
                        CropGranularityX: 1,
                        CropGranularityY: 1,
                        CropAlignX: 0,
                        CropAlignY: 0,
                        MinOutputSize: SIZE {
                            cx: WIDTH,
                            cy: HEIGHT,
                        },
                        MaxOutputSize: SIZE {
                            cx: WIDTH,
                            cy: HEIGHT,
                        },
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
            dbg_log(&format!("IKsPropertySet::Get dwpropid={dwpropid}"));
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

    // -------------------------------------------------------------------
    // IBaseFilter — o filtro em si.
    // -------------------------------------------------------------------

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
            dbg_log("Filter::Stop");
            self.inner.stop_running();
            Ok(())
        }
        fn Pause(&self) -> WinResult<()> {
            dbg_log("Filter::Pause");
            self.inner.ensure_producing(FILTER_STATE(1)); // State_Paused
            Ok(())
        }
        fn Run(&self, _tstart: i64) -> WinResult<()> {
            dbg_log("Filter::Run");
            self.inner.ensure_producing(FILTER_STATE(2)); // State_Running
            Ok(())
        }
        fn GetState(&self, _dwmillisecstimeout: u32) -> WinResult<FILTER_STATE> {
            let s = *self.inner.state.lock().unwrap();
            dbg_log(&format!("Filter::GetState -> {s:?}"));
            Ok(s)
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
            dbg_log("Filter::EnumPins");
            let e: IEnumPins = SinglePinEnum {
                pin: self.pin.clone(),
                index: Mutex::new(0),
            }
            .into();
            Ok(e)
        }
        fn FindPin(&self, _id: &PCWSTR) -> WinResult<IPin> {
            dbg_log("Filter::FindPin");
            Ok(self.pin.clone())
        }
        fn QueryFilterInfo(&self, pinfo: *mut FILTER_INFO) -> WinResult<()> {
            dbg_log("Filter::QueryFilterInfo");
            // SAFETY: `ptr` só é não-None enquanto o grafo dono nos mantém
            // vivos (convenção DirectShow: filtro não faz AddRef do grafo);
            // reconstruímos uma referência EMPRESTADA a partir do ponteiro
            // bruto e clonamos (AddRef de verdade) só a cópia que devolvemos.
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
            dbg_log(&format!(
                "Filter::JoinFilterGraph pgraph_is_some={}",
                pgraph.as_ref().is_some()
            ));
            *self.inner.graph.lock().unwrap() = pgraph.as_ref().map(|g| g.as_raw());
            Ok(())
        }
        fn QueryVendorInfo(&self) -> WinResult<windows::core::PWSTR> {
            Err(E_NOTIMPL.into())
        }
    }

    // -------------------------------------------------------------------
    // IClassFactory + exports da DLL COM.
    // -------------------------------------------------------------------

    #[implement(IClassFactory)]
    struct VCamClassFactory;

    impl IClassFactory_Impl for VCamClassFactory_Impl {
        fn CreateInstance(
            &self,
            punkouter: Ref<'_, IUnknown>,
            riid: *const GUID,
            ppvobject: *mut *mut c_void,
        ) -> WinResult<()> {
            dbg_log("ClassFactory::CreateInstance: begin");
            if punkouter.as_ref().is_some() {
                return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into());
            }
            let vcam = VCamFilter::new();
            dbg_log("ClassFactory::CreateInstance: VCamFilter::new OK");
            let inner = Arc::clone(&vcam.inner);
            let filter: IBaseFilter = vcam.into();
            *inner.self_filter.lock().unwrap() = Some(filter.clone());
            let r = unsafe { filter.query(riid, ppvobject).ok() };
            dbg_log(&format!("ClassFactory::CreateInstance: end -> {r:?}"));
            r
        }
        fn LockServer(&self, _flock: BOOL) -> WinResult<()> {
            Ok(())
        }
    }

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

    #[no_mangle]
    pub unsafe extern "system" fn DllCanUnloadNow() -> HRESULT {
        S_FALSE
    }

    // -------------------------------------------------------------------
    // Registro: escreve em HKEY_CURRENT_USER\Software\Classes — sem
    // elevação (o Windows mescla essa hive sobre HKEY_CLASSES_ROOT para o
    // mesmo usuário na hora da leitura, então Chrome/OBS enxergam o
    // filtro normalmente).
    // -------------------------------------------------------------------

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

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
}
