//! Finding and opening the writer on Windows.
//!
//! SetupAPI to turn a device interface GUID into a device path, then plain
//! synchronous `CreateFile`/`ReadFile`/`WriteFile`. No overlapped I/O, no
//! ioctls.

use std::ffi::CStr;
use std::mem::{offset_of, size_of};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_A, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsA, SetupDiGetDeviceInterfaceDetailA,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, GENERIC_READ, GENERIC_WRITE,
    HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
    WriteFile,
};
use windows::core::{GUID, PCSTR};

use crate::machine::MachineError;
use crate::stenograph::packet::{self, HEADER_LEN, MAX_READ, Response};

/// The writer's device interface class.
///
/// **Not** the GUID spelled in `plover-stenograph`'s source. That code builds
/// its GUID from `uuid.UUID(...).bytes`, which is big endian, and hands the raw
/// 16 bytes to Win32, whose `GUID` is little endian in its first three fields.
/// The string therefore arrives byte swapped, and the swapped form is the real
/// one. Confirmed against this machine's registry, where the Luminex's
/// `MI_00` interface is registered under exactly this value:
/// `HKLM\SYSTEM\CurrentControlSet\Control\DeviceClasses`.
///
/// Using the unswapped string enumerates nothing, and the failure is
/// indistinguishable from the writer being switched off.
const WRITER_INTERFACE: GUID = GUID::from_u128(0x202e68c5_5980_4a60_b761_77c4de9d5dbf);

/// Big enough for a full response: header plus the largest read we ask for.
const READ_BUFFER_LEN: usize = HEADER_LEN + MAX_READ as usize;

/// Releases the enumeration handle however we leave the function.
struct DeviceInfoList(HDEVINFO);

impl Drop for DeviceInfoList {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn io_error(context: &str, error: windows::core::Error) -> MachineError {
    MachineError::Io(format!("{context}: {error}"))
}

/// The device path of the first attached writer.
fn find_device_path() -> Result<String, MachineError> {
    let devinfo = unsafe {
        SetupDiGetClassDevsA(
            Some(&WRITER_INTERFACE),
            PCSTR::null(),
            None,
            DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
        )
    }
    .map_err(|e| io_error("SetupDiGetClassDevs", e))?;
    let devinfo = DeviceInfoList(devinfo);

    let mut interface = SP_DEVICE_INTERFACE_DATA {
        cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
        ..Default::default()
    };

    // Member index 0: we only ever talk to one writer.
    if let Err(e) = unsafe {
        SetupDiEnumDeviceInterfaces(devinfo.0, None, &WRITER_INTERFACE, 0, &mut interface)
    } {
        // No writer plugged in. The normal idle state, once per second forever
        // while the machine is off, so it must stay quiet and retryable.
        if e.code() == ERROR_NO_MORE_ITEMS.to_hresult() {
            return Err(MachineError::NotAttached);
        }
        return Err(io_error("SetupDiEnumDeviceInterfaces", e));
    }

    // The first call is *expected* to fail with ERROR_INSUFFICIENT_BUFFER: a
    // null buffer is how you ask for the required size. Only another error is
    // a real one.
    let mut required: u32 = 0;
    if let Err(e) = unsafe {
        SetupDiGetDeviceInterfaceDetailA(devinfo.0, &interface, None, 0, Some(&mut required), None)
    } && e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
    {
        return Err(io_error("SetupDiGetDeviceInterfaceDetail (sizing)", e));
    }
    if (required as usize) <= offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_A, DevicePath) {
        return Err(MachineError::Io(format!(
            "SetupDiGetDeviceInterfaceDetail asked for {required} bytes, too small for a path"
        )));
    }

    // A Vec<u32> so the buffer is 4 byte aligned: cbSize is written through
    // this pointer, and a misaligned u32 write is undefined behaviour.
    let mut buffer: Vec<u32> = vec![0; required.div_ceil(4) as usize];
    let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_A;

    // cbSize is the size of the struct itself, never the allocated buffer:
    // 8 on x64 (a DWORD plus one CHAR, padded), 5 on x86 where it is packed.
    // size_of gets both right. Passing the buffer size yields
    // ERROR_INVALID_USER_BUFFER, whose message hints at nothing.
    unsafe {
        (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_A>() as u32;
    }

    unsafe {
        SetupDiGetDeviceInterfaceDetailA(devinfo.0, &interface, Some(detail), required, None, None)
    }
    .map_err(|e| io_error("SetupDiGetDeviceInterfaceDetail", e))?;

    // The path is a NUL terminated ANSI string starting at the DevicePath
    // field, which sits at offset 4 whatever the struct's padded size is.
    let path_start = unsafe {
        buffer
            .as_ptr()
            .cast::<u8>()
            .add(offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_A, DevicePath))
    };
    let path = unsafe { CStr::from_ptr(path_start.cast()) };

    path.to_str()
        .map(str::to_owned)
        .map_err(|e| MachineError::Io(format!("device path is not valid UTF-8: {e}")))
}

/// An open handle to the writer.
pub struct Transport {
    handle: HANDLE,
    path: String,
    buffer: Vec<u8>,
}

// A Windows handle is process wide and carries no thread affinity. This one is
// only ever touched from the machine thread; the wrapper moves there once.
unsafe impl Send for Transport {}

impl Transport {
    /// Open the first attached writer.
    ///
    /// [`MachineError::NotAttached`] means the writer is off or unplugged,
    /// which is ordinary and is what the scanner retries on.
    pub fn open() -> Result<Transport, MachineError> {
        let path = find_device_path()?;

        let device_path = std::ffi::CString::new(path.as_str())
            .map_err(|e| MachineError::Io(format!("device path has an interior NUL: {e}")))?;

        let handle = unsafe {
            CreateFileA(
                PCSTR(device_path.as_ptr().cast()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                // Plover passes CREATE_ALWAYS | CREATE_NEW, which is 2 | 1 == 3,
                // and 3 happens to be OPEN_EXISTING. It works by numeric
                // coincidence. Say what we mean instead.
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| io_error("CreateFile", e))?;

        Ok(Transport {
            handle,
            path,
            buffer: vec![0; READ_BUFFER_LEN],
        })
    }

    /// The device path, for the status bar.
    pub fn path(&self) -> &str {
        &self.path
    }

    fn write_packet(&mut self, request: &[u8]) -> Result<(), MachineError> {
        let mut written: u32 = 0;
        unsafe { WriteFile(self.handle, Some(request), Some(&mut written), None) }
            .map_err(|e| io_error("WriteFile", e))?;

        if written as usize != request.len() {
            return Err(MachineError::Io(format!(
                "short write to the writer: {written} of {} bytes",
                request.len()
            )));
        }
        Ok(())
    }

    /// Read one response. Header and payload arrive in a single read.
    fn read_packet(&mut self) -> Result<(Response, Vec<u8>), MachineError> {
        let mut read: u32 = 0;
        unsafe { ReadFile(self.handle, Some(&mut self.buffer), Some(&mut read), None) }
            .map_err(|e| io_error("ReadFile", e))?;

        let read = read as usize;
        if read < HEADER_LEN {
            return Err(MachineError::Io(format!(
                "short read from the writer: {read} of {HEADER_LEN} header bytes"
            )));
        }

        let response = packet::decode_header(&self.buffer)?;

        let available = read - HEADER_LEN;
        let wanted = response.data_length as usize;
        if wanted > available {
            return Err(MachineError::Protocol(format!(
                "response claims {wanted} payload bytes but only {available} arrived"
            )));
        }

        let payload = self.buffer[HEADER_LEN..HEADER_LEN + wanted].to_vec();
        Ok((response, payload))
    }

    /// Send a request and read its response.
    pub fn send_receive(
        &mut self,
        request: &[u8],
    ) -> Result<(Response, Vec<u8>), MachineError> {
        self.write_packet(request)?;
        self.read_packet()
    }
}

impl Drop for Transport {
    /// Take the handle out and *then* close it.
    ///
    /// Plover's `disconnect()` assigns INVALID_HANDLE_VALUE first and closes
    /// that, so the real handle leaks and the close fails. Across a retry loop
    /// that exhausts handles. The M4b soak test watches for it.
    fn drop(&mut self) {
        let handle = std::mem::replace(&mut self.handle, HANDLE(std::ptr::null_mut()));
        if !handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }
}
