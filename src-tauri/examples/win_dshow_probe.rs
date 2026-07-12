//! Diagnóstico do Spike C (T074): enumera dispositivos DirectShow na
//! categoria de captura de vídeo e tenta instanciar/inspecionar cada um,
//! isolando exatamente qual chamada falha (ao contrário do ffmpeg, que só
//! reporta "segmentation fault" sem indicar o ponto exato).

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("win_dshow_probe é específico do Windows.");
}

#[cfg(target_os = "windows")]
fn main() -> windows::core::Result<()> {
    probe::run()
}

#[cfg(target_os = "windows")]
mod probe {
    use std::io::Write;

    use windows::core::{Interface, Result as WinResult, GUID};
    use windows::Win32::Media::DirectShow::{
        IAMStreamConfig, IBaseFilter, ICreateDevEnum, IEnumPins, IPin, PIN_INFO,
        VIDEO_STREAM_CONFIG_CAPS,
    };
    use windows::Win32::Media::KernelStreaming::IKsPropertySet;
    use windows::Win32::Media::MediaFoundation::{
        CLSID_SystemDeviceEnum, CLSID_VideoInputDeviceCategory,
    };
    use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IEnumMoniker, IMoniker,
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Variant::VARIANT;

    const AMPROPSETID_PIN: GUID = GUID::from_u128(0x9b00f101_1567_11d1_b3f1_00aa003761c5);
    const AMPROPERTY_PIN_CATEGORY: u32 = 0;

    pub fn run() -> WinResult<()> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

            let dev_enum: ICreateDevEnum =
                CoCreateInstance(&CLSID_SystemDeviceEnum, None, CLSCTX_INPROC_SERVER)?;
            let mut enum_moniker: Option<IEnumMoniker> = None;
            dev_enum.CreateClassEnumerator(
                &CLSID_VideoInputDeviceCategory,
                &mut enum_moniker,
                0,
            )?;

            let Some(enum_moniker) = enum_moniker else {
                println!("Nenhum dispositivo de captura de vídeo registrado.");
                CoUninitialize();
                return Ok(());
            };

            let mut count = 0;
            loop {
                let mut monikers: [Option<IMoniker>; 1] = [None];
                let mut fetched = 0u32;
                let hr = enum_moniker.Next(&mut monikers, Some(&mut fetched));
                if hr.is_err() || fetched == 0 {
                    break;
                }
                count += 1;
                let Some(moniker) = monikers[0].take() else {
                    continue;
                };
                inspect_device(count, &moniker);
            }

            println!("Total de devices na categoria: {count}");
            CoUninitialize();
        }
        Ok(())
    }

    fn flush() {
        std::io::stdout().flush().ok();
    }

    unsafe fn inspect_device(index: u32, moniker: &IMoniker) {
        println!("--- device #{index} ---");

        print!("BindToStorage::<IPropertyBag> ... ");
        flush();
        let prop_bag: WinResult<IPropertyBag> = moniker.BindToStorage(None, None);
        match prop_bag {
            Ok(prop_bag) => {
                println!("OK");
                print!("Read(FriendlyName) ... ");
                flush();
                let mut var = VARIANT::default();
                match prop_bag.Read(windows::core::w!("FriendlyName"), &mut var, None) {
                    Ok(_) => println!("OK -> {:?}", var.to_string()),
                    Err(e) => println!("falhou: {e:?}"),
                }
            }
            Err(e) => println!("BindToStorage falhou: {e:?}"),
        }

        print!("BindToObject::<IBaseFilter> ... ");
        flush();
        let filter: WinResult<IBaseFilter> = moniker.BindToObject(None, None);
        let Ok(filter) = filter else {
            println!("falhou: {:?}", filter.unwrap_err());
            return;
        };
        println!("OK");

        print!("EnumPins ... ");
        flush();
        match filter.EnumPins() {
            Ok(enum_pins) => {
                println!("OK");
                inspect_pins(&enum_pins);
            }
            Err(e) => println!("falhou: {e:?}"),
        }
    }

    unsafe fn inspect_pins(enum_pins: &IEnumPins) {
        let mut pin_index = 0;
        loop {
            let mut pins: [Option<IPin>; 1] = [None];
            let mut fetched = 0u32;
            let hr = enum_pins.Next(&mut pins, Some(&mut fetched));
            if hr.is_err() || fetched == 0 {
                break;
            }
            let Some(pin) = pins[0].take() else {
                continue;
            };
            pin_index += 1;
            println!("  -- pin #{pin_index} --");
            inspect_pin(&pin);
        }
    }

    unsafe fn inspect_pin(pin: &IPin) {
        print!("  QueryPinInfo ... ");
        flush();
        let mut info = PIN_INFO::default();
        match pin.QueryPinInfo(&mut info) {
            Ok(_) => println!("OK (dir={:?})", info.dir),
            Err(e) => println!("falhou: {e:?}"),
        }

        print!("  QueryDirection ... ");
        flush();
        match pin.QueryDirection() {
            Ok(d) => println!("OK ({d:?})"),
            Err(e) => println!("falhou: {e:?}"),
        }

        print!("  QueryId ... ");
        flush();
        match pin.QueryId() {
            Ok(id) => println!("OK ({:?})", id.to_string()),
            Err(e) => println!("falhou: {e:?}"),
        }

        print!("  EnumMediaTypes ... ");
        flush();
        match pin.EnumMediaTypes() {
            Ok(enum_mt) => {
                println!("OK");
                let mut mts = [std::ptr::null_mut(); 1];
                let mut fetched = 0u32;
                let hr = enum_mt.Next(&mut mts, Some(&mut fetched));
                println!("    Next -> hr={hr:?} fetched={fetched}");
            }
            Err(e) => println!("falhou: {e:?}"),
        }

        print!("  IKsPropertySet::Get(PIN_CATEGORY) ... ");
        flush();
        match pin.cast::<IKsPropertySet>() {
            Ok(ks) => {
                let mut buf = [0u8; 16];
                let mut returned = 0u32;
                match ks.Get(
                    &AMPROPSETID_PIN,
                    AMPROPERTY_PIN_CATEGORY,
                    std::ptr::null(),
                    0,
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                    &mut returned,
                ) {
                    Ok(_) => println!("OK (returned={returned})"),
                    Err(e) => println!("falhou: {e:?}"),
                }
            }
            Err(e) => println!("cast IKsPropertySet falhou: {e:?}"),
        }

        print!("  cast IAMStreamConfig + GetNumberOfCapabilities ... ");
        flush();
        match pin.cast::<IAMStreamConfig>() {
            Ok(cfg) => {
                let mut count_caps = 0i32;
                let mut size_caps = 0i32;
                match cfg.GetNumberOfCapabilities(&mut count_caps, &mut size_caps) {
                    Ok(_) => {
                        println!("OK (count={count_caps} size={size_caps})");
                        print!("  GetStreamCaps(0) ... ");
                        flush();
                        let mut mt_ptr = std::ptr::null_mut();
                        let mut caps = VIDEO_STREAM_CONFIG_CAPS::default();
                        match cfg.GetStreamCaps(0, &mut mt_ptr, &mut caps as *mut _ as *mut u8) {
                            Ok(_) => println!("OK"),
                            Err(e) => println!("falhou: {e:?}"),
                        }
                    }
                    Err(e) => println!("falhou: {e:?}"),
                }
            }
            Err(e) => println!("cast IAMStreamConfig falhou: {e:?}"),
        }
    }
}
