//! One frame of one window through Windows.Graphics.Capture: the window's composed content as the compositor holds it,
//! Direct3D swap chains included. The capture item is the window handle, never a monitor.

use std::time::{Duration, Instant};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{E_FAIL, HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{Error, Interface, Result, factory};

/// A hardware D3D11 device with BGRA support (what WGC needs) and its immediate context.
pub fn device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut d, mut c) = (None, None);
    // SAFETY: out-pointers to locals that live through the call.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d),
            None,
            Some(&mut c),
        )?
    };
    d.zip(c).ok_or_else(Error::empty)
}

/// The window's pixels as (width, height, RGBA): one frame, no yellow border or cursor where the OS allows.
pub fn grab(h: windows_sys::Win32::Foundation::HWND) -> Result<(u32, u32, Vec<u8>)> {
    if !GraphicsCaptureSession::IsSupported()? {
        return Err(Error::new(E_FAIL, "WGC not supported"));
    }
    // SAFETY: COM/WinRT calls on objects created here; `h` is a window handle (CreateForWindow checks it).
    unsafe {
        let item: GraphicsCaptureItem =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?
                .CreateForWindow(HWND(h))?;
        let (dev, ctx) = device()?;
        let rt: IDirect3DDevice =
            CreateDirect3D11DeviceFromDXGIDevice(&dev.cast::<IDXGIDevice>()?)?.cast()?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &rt,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        // both setters are newer than WGC itself (Windows 10 2004 / 11): an older OS keeps the border or cursor
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        session.StartCapture()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let frame = loop {
            if let Ok(f) = pool.TryGetNextFrame() {
                break f;
            }
            if Instant::now() > deadline {
                session.Close()?;
                return Err(Error::new(E_FAIL, "no frame within 3 s"));
            }
            std::thread::sleep(Duration::from_millis(15));
        };
        let tex: ID3D11Texture2D = frame
            .Surface()?
            .cast::<IDirect3DDxgiInterfaceAccess>()?
            .GetInterface()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        dev.CreateTexture2D(&desc, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(Error::empty)?;
        ctx.CopyResource(&staging, &tex);
        // the frame can be smaller than the pool when the window shrank since; keep its own content size
        let cs = frame.ContentSize()?;
        let (w, ht) = (
            (cs.Width as u32).min(desc.Width),
            (cs.Height as u32).min(desc.Height),
        );
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let mut px = Vec::with_capacity((w * ht * 4) as usize);
        for y in 0..ht as usize {
            let row = std::slice::from_raw_parts(
                (m.pData as *const u8).add(y * m.RowPitch as usize),
                w as usize * 4,
            );
            px.extend_from_slice(row);
        }
        ctx.Unmap(&staging, 0);
        frame.Close()?;
        session.Close()?;
        pool.Close()?;
        for p in px.as_chunks_mut::<4>().0 {
            p.swap(0, 2); // BGRA -> RGBA
            p[3] = 255;
        }
        Ok((w, ht, px))
    }
}
