use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_FILE_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, WRITE_DAC,
};

pub(super) fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(GENERIC_READ | WRITE_DAC)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

pub(super) fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(GENERIC_READ | GENERIC_WRITE | WRITE_DAC)
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

pub(super) fn restrict(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration must be a regular file",
        ));
    }

    let policy: Vec<u16> = "D:P(A;;FA;;;OW)(A;;FA;;;SY)"
        .encode_utf16()
        .chain([0])
        .collect();
    let mut descriptor = null_mut();
    // The policy is NUL-terminated. Windows allocates the resulting descriptor.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            policy.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    let (mut present, mut defaulted, mut dacl) = (0, 0, null_mut());
    // DACL storage belongs to the descriptor and is live through SetSecurityInfo.
    let result = unsafe {
        if GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) == 0 {
            Err(io::Error::last_os_error())
        } else if present == 0 || dacl.is_null() {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing configuration ACL",
            ))
        } else {
            let status = SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null(),
            );
            if status == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(status as i32))
            }
        }
    };
    // SetSecurityInfo has copied the policy.
    unsafe { LocalFree(descriptor.cast()) };
    result
}
