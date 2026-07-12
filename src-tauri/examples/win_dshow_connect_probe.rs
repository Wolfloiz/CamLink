//! Diagnóstico do Spike C (T074): monta um grafo DirectShow mínimo
//! (nosso filtro → Video Renderer) via `IGraphBuilder`, exatamente o tipo
//! de negociação de pino/allocator que Chrome/OBS fazem ao abrir a
//! câmera de verdade — isola falhas de `Connect`/`Run` fora do processo
//! do Chrome (onde não temos stdout).

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("win_dshow_connect_probe é específico do Windows.");
}

#[cfg(target_os = "windows")]
fn main() -> windows::core::Result<()> {
    use std::io::Write;
    use std::thread;
    use std::time::Duration;

    use windows::core::{Interface, GUID};
    use windows::Win32::Media::DirectShow::{
        IBaseFilter, IEnumPins, IFilterGraph2, IGraphBuilder, IMediaControl, IPin, State_Running,
        PINDIR_INPUT,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    // Um app STA de verdade (Chrome/OBS) tem um loop de mensagens rodando no
    // thread principal — é isso que permite ao COM entregar chamadas
    // marshaladas de volta pra essa apartment (ex.: nosso worker thread
    // chamando IMemInputPin::Receive através da Global Interface Table). Um
    // `thread::sleep` puro NÃO bombeia mensagens e simula um consumidor mal
    // comportado — por isso pumpeamos aqui, para testar o cenário real.
    fn pump_for(duration: Duration) {
        let deadline = std::time::Instant::now() + duration;
        let mut msg = MSG::default();
        while std::time::Instant::now() < deadline {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    const CLSID_CAMLINK_FILTER: GUID = GUID::from_u128(0xe7a04118_a6bc_496d_9325_c0e2d265a006);
    const CLSID_FILTER_GRAPH: GUID = GUID::from_u128(0xe436ebb3_524f_11ce_9f53_0020af0ba770);
    // Null Renderer: descarta samples, sem janela/GDI — isola bugs do
    // Video Renderer legado do nosso próprio filtro.
    const CLSID_NULL_RENDERER: GUID = GUID::from_u128(0xc1f400a4_3f08_11d3_9f0b_006008039e37);

    fn flush() {
        std::io::stdout().flush().ok();
    }

    unsafe fn first_pin_with_dir(
        filter: &IBaseFilter,
        want_input: bool,
    ) -> windows::core::Result<IPin> {
        let enum_pins: IEnumPins = filter.EnumPins()?;
        loop {
            let mut pins: [Option<IPin>; 1] = [None];
            let mut fetched = 0u32;
            let hr = enum_pins.Next(&mut pins, Some(&mut fetched));
            if hr.is_err() || fetched == 0 {
                return Err(windows::core::Error::from(
                    windows::Win32::Foundation::E_FAIL,
                ));
            }
            if let Some(pin) = pins[0].take() {
                let dir = pin.QueryDirection()?;
                let is_input = dir == PINDIR_INPUT;
                if is_input == want_input {
                    return Ok(pin);
                }
            }
        }
    }

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

        print!("CoCreateInstance(FilterGraph) ... ");
        flush();
        let graph: IGraphBuilder =
            CoCreateInstance(&CLSID_FILTER_GRAPH, None, CLSCTX_INPROC_SERVER)?;
        println!("OK");

        print!("CoCreateInstance(CamLink filter) ... ");
        flush();
        let camlink: IBaseFilter =
            CoCreateInstance(&CLSID_CAMLINK_FILTER, None, CLSCTX_INPROC_SERVER)?;
        println!("OK");

        print!("graph.AddFilter(camlink) ... ");
        flush();
        let graph2: IFilterGraph2 = graph.cast()?;
        graph2.AddFilter(&camlink, windows::core::w!("CamLink"))?;
        println!("OK");

        print!("CoCreateInstance(NullRenderer) ... ");
        flush();
        let renderer: IBaseFilter =
            CoCreateInstance(&CLSID_NULL_RENDERER, None, CLSCTX_INPROC_SERVER)?;
        println!("OK");

        print!("graph.AddFilter(renderer) ... ");
        flush();
        graph2.AddFilter(&renderer, windows::core::w!("Renderer"))?;
        println!("OK");

        print!("camlink output pin ... ");
        flush();
        let out_pin = first_pin_with_dir(&camlink, false)?;
        println!("OK");

        print!("renderer input pin ... ");
        flush();
        let in_pin = first_pin_with_dir(&renderer, true)?;
        println!("OK");

        print!("graph.Connect(out_pin, in_pin) ... ");
        flush();
        match graph.Connect(&out_pin, &in_pin) {
            Ok(_) => println!("OK"),
            Err(e) => {
                println!("FALHOU: {e:?}");
                CoUninitialize();
                return Ok(());
            }
        }

        print!("IMediaControl::Run ... ");
        flush();
        let control: IMediaControl = graph.cast()?;
        match control.Run() {
            Ok(_) => println!("OK"),
            Err(e) => {
                println!("FALHOU: {e:?}");
                CoUninitialize();
                return Ok(());
            }
        }

        pump_for(Duration::from_millis(500));
        match control.GetState(1000) {
            Ok(state) => println!("GetState -> {state} (Running={})", state == State_Running.0),
            Err(e) => println!("GetState falhou: {e:?}"),
        }

        pump_for(Duration::from_secs(2));
        let _ = control.Stop();
        println!("Stop() chamado. Fim.");

        CoUninitialize();
    }
    Ok(())
}
