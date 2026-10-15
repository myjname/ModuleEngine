//! Native Windows dialogs used by the editor's background command worker.
//!
//! Each call is modal on its calling thread, so the editor must invoke it from
//! its single dialog worker rather than the UI thread. Cancellation is `Ok(None)`.
//! Windows' common item dialog returns an owned filesystem path without a fixed
//! `MAX_PATH` buffer. No process current directory is changed and no helper
//! process or console is started.

use std::path::{Path, PathBuf};

/// Choose an existing ModuleEngine project JSON file.
pub fn open_project(initial_dir: &Path) -> Result<Option<PathBuf>, String> {
    platform::show(
        DialogKind::OpenProject,
        initial_dir,
        None,
        "Open ModuleEngine Project",
    )
}

/// Choose a project JSON destination, with the native overwrite confirmation.
pub fn save_project(initial_path: &Path) -> Result<Option<PathBuf>, String> {
    let directory = initial_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    platform::show(
        DialogKind::SaveProject,
        directory,
        initial_path.file_name(),
        "Save Project As",
    )
}

/// Choose a filesystem folder, for example an empty package destination.
pub fn pick_folder(title: &str, initial_dir: &Path) -> Result<Option<PathBuf>, String> {
    platform::show(DialogKind::Folder, initial_dir, None, title)
}

/// Choose one model, texture, or audio source supported by the current importer.
pub fn import_asset(initial_dir: &Path) -> Result<Option<PathBuf>, String> {
    platform::show(DialogKind::ImportAsset, initial_dir, None, "Import Asset")
}

#[derive(Clone, Copy)]
enum DialogKind {
    OpenProject,
    SaveProject,
    Folder,
    ImportAsset,
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    use std::ffi::OsStr;

    pub(super) fn show(
        _kind: DialogKind,
        _initial_dir: &Path,
        _file_name: Option<&OsStr>,
        _title: &str,
    ) -> Result<Option<PathBuf>, String> {
        Err("Native file and folder dialogs are currently available only on Windows.".into())
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        ffi::{OsStr, OsString, c_void},
        os::windows::ffi::{OsStrExt, OsStringExt},
        ptr::{self, NonNull},
    };

    type Hresult = i32;
    const CANCELLED: Hresult = 0x8007_04c7_u32 as i32;
    const APARTMENT_THREADED: u32 = 2;
    const INPROC_SERVER: u32 = 1;
    const FORCE_FILESYSTEM: u32 = 0x40;
    const NO_CHANGE_DIR: u32 = 0x08;
    const PATH_MUST_EXIST: u32 = 0x800;
    const FILE_MUST_EXIST: u32 = 0x1000;
    const PICK_FOLDERS: u32 = 0x20;
    const OVERWRITE_PROMPT: u32 = 0x02;
    const STRICT_FILE_TYPES: u32 = 0x04;
    const FILESYSTEM_PATH: i32 = 0x8005_8000_u32 as i32;

    // IDs and ABI layout from the Windows SDK's ShObjIdl_core.h. Only the common
    // IFileDialog methods are used, for both FileOpenDialog and FileSaveDialog.
    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const OPEN_DIALOG: Guid = Guid {
        data1: 0xdc1c5a9c,
        data2: 0xe88a,
        data3: 0x4dde,
        data4: [0xa5, 0xa1, 0x60, 0xf8, 0x2a, 0x20, 0xae, 0xf7],
    };
    const SAVE_DIALOG: Guid = Guid {
        data1: 0xc0b4e2f3,
        data2: 0xba21,
        data3: 0x4773,
        data4: [0x8d, 0xba, 0x33, 0x5e, 0xc9, 0x46, 0xeb, 0x8b],
    };
    const FILE_DIALOG: Guid = Guid {
        data1: 0x42f85136,
        data2: 0xdb7e,
        data3: 0x439c,
        data4: [0x85, 0xf1, 0xe4, 0x07, 0x5d, 0x13, 0x5f, 0xc8],
    };
    const SHELL_ITEM: Guid = Guid {
        data1: 0x43826d1e,
        data2: 0xe718,
        data3: 0x42ee,
        data4: [0xbc, 0x55, 0xa1, 0xe2, 0x61, 0xc3, 0x7b, 0xfe],
    };

    #[repr(C)]
    struct Unknown {
        vtable: *const UnknownVtable,
    }
    #[repr(C)]
    struct UnknownVtable {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut Unknown) -> u32,
    }
    #[repr(C)]
    struct FileDialog {
        vtable: *const FileDialogVtable,
    }
    #[repr(C)]
    struct FileDialogVtable {
        unknown: UnknownVtable,
        show: unsafe extern "system" fn(*mut FileDialog, *mut c_void) -> Hresult,
        set_file_types:
            unsafe extern "system" fn(*mut FileDialog, u32, *const FilterSpec) -> Hresult,
        set_file_type_index: usize,
        get_file_type_index: usize,
        advise: usize,
        unadvise: usize,
        set_options: unsafe extern "system" fn(*mut FileDialog, u32) -> Hresult,
        get_options: unsafe extern "system" fn(*mut FileDialog, *mut u32) -> Hresult,
        set_default_folder: usize,
        set_folder: unsafe extern "system" fn(*mut FileDialog, *mut ShellItem) -> Hresult,
        get_folder: usize,
        get_current_selection: usize,
        set_file_name: unsafe extern "system" fn(*mut FileDialog, *const u16) -> Hresult,
        get_file_name: usize,
        set_title: unsafe extern "system" fn(*mut FileDialog, *const u16) -> Hresult,
        set_ok_button_label: usize,
        set_file_name_label: usize,
        get_result: unsafe extern "system" fn(*mut FileDialog, *mut *mut ShellItem) -> Hresult,
        add_place: usize,
        set_default_extension: unsafe extern "system" fn(*mut FileDialog, *const u16) -> Hresult,
        close: usize,
        set_client_guid: usize,
        clear_client_data: usize,
        set_filter: usize,
    }
    #[repr(C)]
    struct ShellItem {
        vtable: *const ShellItemVtable,
    }
    #[repr(C)]
    struct ShellItemVtable {
        unknown: UnknownVtable,
        bind_to_handler: usize,
        get_parent: usize,
        get_display_name: unsafe extern "system" fn(*mut ShellItem, i32, *mut *mut u16) -> Hresult,
        get_attributes: usize,
        compare: usize,
    }
    #[repr(C)]
    struct FilterSpec {
        name: *const u16,
        pattern: *const u16,
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *const c_void, flags: u32) -> Hresult;
        fn CoUninitialize();
        fn CoCreateInstance(
            class: *const Guid,
            outer: *mut c_void,
            context: u32,
            interface: *const Guid,
            result: *mut *mut c_void,
        ) -> Hresult;
        fn CoTaskMemFree(memory: *const c_void);
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHCreateItemFromParsingName(
            path: *const u16,
            bind_context: *mut c_void,
            interface: *const Guid,
            result: *mut *mut c_void,
        ) -> Hresult;
    }

    struct Apartment;
    impl Apartment {
        fn initialize() -> Result<Self, String> {
            // A fresh editor worker has no COM apartment yet. S_FALSE also
            // increments the COM initialization count and needs balancing.
            check(
                unsafe { CoInitializeEx(ptr::null(), APARTMENT_THREADED) },
                "Initialize dialog worker",
            )?;
            Ok(Self)
        }
    }
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    struct OwnedCom<T>(NonNull<T>);
    impl<T> OwnedCom<T> {
        fn take(pointer: *mut T, operation: &str) -> Result<Self, String> {
            NonNull::new(pointer)
                .map(Self)
                .ok_or_else(|| format!("{operation} returned no COM object."))
        }
        fn pointer(&self) -> *mut T {
            self.0.as_ptr()
        }
    }
    impl<T> Drop for OwnedCom<T> {
        fn drop(&mut self) {
            // Every interface has the IUnknown vtable prefix. All guards are
            // dropped on this same worker before its apartment is released.
            unsafe {
                let unknown = self.pointer().cast::<Unknown>();
                ((*(*unknown).vtable).release)(unknown);
            }
        }
    }

    fn check(result: Hresult, operation: &str) -> Result<(), String> {
        if result >= 0 {
            return Ok(());
        }
        let code = result as u32;
        if code >> 16 == 0x8007 {
            let detail = std::io::Error::from_raw_os_error((code & 0xffff) as i32);
            Err(format!("{operation} failed (0x{code:08X}): {detail}"))
        } else {
            Err(format!("{operation} failed (0x{code:08X})."))
        }
    }

    fn wide(value: &OsStr) -> Result<Vec<u16>, String> {
        let mut encoded: Vec<_> = value.encode_wide().collect();
        if encoded.contains(&0) {
            return Err("Dialog text or path contains a null character.".into());
        }
        encoded.push(0);
        Ok(encoded)
    }

    fn initial_folder(path: &Path) -> Result<Option<OwnedCom<ShellItem>>, String> {
        // A moved/deleted recent directory should not prevent opening a dialog.
        if !path.is_dir() {
            return Ok(None);
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| format!("Read current directory: {e}"))?
                .join(path)
        };
        let encoded = wide(absolute.as_os_str())?;
        let mut item = ptr::null_mut();
        check(
            unsafe {
                SHCreateItemFromParsingName(
                    encoded.as_ptr(),
                    ptr::null_mut(),
                    &SHELL_ITEM,
                    &mut item,
                )
            },
            "Open initial dialog folder",
        )?;
        OwnedCom::take(item.cast::<ShellItem>(), "Open initial dialog folder").map(Some)
    }

    fn selected_path(dialog: &OwnedCom<FileDialog>) -> Result<PathBuf, String> {
        let mut item = ptr::null_mut();
        let vtable = unsafe { &*(*dialog.pointer()).vtable };
        check(
            unsafe { (vtable.get_result)(dialog.pointer(), &mut item) },
            "Read selected item",
        )?;
        let item = OwnedCom::take(item, "Read selected item")?;
        shell_path(&item)
    }

    fn shell_path(item: &OwnedCom<ShellItem>) -> Result<PathBuf, String> {
        let mut path = ptr::null_mut();
        let item_vtable = unsafe { &*(*item.pointer()).vtable };
        check(
            unsafe { (item_vtable.get_display_name)(item.pointer(), FILESYSTEM_PATH, &mut path) },
            "Read selected filesystem path",
        )?;
        let path = NonNull::new(path).ok_or("Selected item has no filesystem path.")?;
        // The Shell allocates this null-terminated string; always free it after
        // copying. Unlike the old common dialogs, no fixed path buffer is used.
        let mut length = 0;
        unsafe {
            while *path.as_ptr().add(length) != 0 {
                length += 1;
            }
            let result = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(
                path.as_ptr(),
                length,
            )));
            CoTaskMemFree(path.as_ptr().cast());
            Ok(result)
        }
    }

    pub(super) fn show(
        kind: DialogKind,
        initial_dir: &Path,
        file_name: Option<&OsStr>,
        title: &str,
    ) -> Result<Option<PathBuf>, String> {
        // Keep all backing strings alive until after the COM dialog is released,
        // including every early-return path during configuration or cancellation.
        let title = wide(OsStr::new(title))?;
        let file_name = file_name.map(wide).transpose()?;
        let extension = wide(OsStr::new("json"))?;
        let filters: &[(&str, &str)] = match kind {
            DialogKind::Folder => &[],
            DialogKind::OpenProject | DialogKind::SaveProject => {
                &[("ModuleEngine project (*.json)", "*.json")]
            }
            DialogKind::ImportAsset => &[
                ("Supported assets", "*.gltf;*.glb;*.png;*.jpg;*.jpeg;*.wav"),
                ("3D models (*.gltf;*.glb)", "*.gltf;*.glb"),
                ("Textures (*.png;*.jpg;*.jpeg)", "*.png;*.jpg;*.jpeg"),
                ("Audio (*.wav)", "*.wav"),
            ],
        };
        let filter_text: Vec<_> = filters
            .iter()
            .map(|(name, pattern)| Ok((wide(OsStr::new(name))?, wide(OsStr::new(pattern))?)))
            .collect::<Result<_, String>>()?;
        let filter_specs: Vec<_> = filter_text
            .iter()
            .map(|(name, pattern)| FilterSpec {
                name: name.as_ptr(),
                pattern: pattern.as_ptr(),
            })
            .collect();
        let _apartment = Apartment::initialize()?;
        let class = if matches!(kind, DialogKind::SaveProject) {
            &SAVE_DIALOG
        } else {
            &OPEN_DIALOG
        };
        let mut dialog = ptr::null_mut();
        check(
            unsafe {
                CoCreateInstance(
                    class,
                    ptr::null_mut(),
                    INPROC_SERVER,
                    &FILE_DIALOG,
                    &mut dialog,
                )
            },
            "Create native dialog",
        )?;
        let dialog = OwnedCom::take(dialog.cast::<FileDialog>(), "Create native dialog")?;
        let vtable = unsafe { &*(*dialog.pointer()).vtable };
        let mut options = 0;
        check(
            unsafe { (vtable.get_options)(dialog.pointer(), &mut options) },
            "Read native dialog options",
        )?;
        options |= FORCE_FILESYSTEM | NO_CHANGE_DIR | PATH_MUST_EXIST;
        options |= match kind {
            DialogKind::Folder => PICK_FOLDERS,
            DialogKind::SaveProject => OVERWRITE_PROMPT | STRICT_FILE_TYPES,
            DialogKind::OpenProject | DialogKind::ImportAsset => FILE_MUST_EXIST,
        };
        check(
            unsafe { (vtable.set_options)(dialog.pointer(), options) },
            "Set native dialog options",
        )?;
        check(
            unsafe { (vtable.set_title)(dialog.pointer(), title.as_ptr()) },
            "Set native dialog title",
        )?;
        if let Some(folder) = initial_folder(initial_dir)? {
            check(
                unsafe { (vtable.set_folder)(dialog.pointer(), folder.pointer()) },
                "Set initial dialog folder",
            )?;
        }
        if let Some(file_name) = file_name.as_ref() {
            check(
                unsafe { (vtable.set_file_name)(dialog.pointer(), file_name.as_ptr()) },
                "Set initial dialog filename",
            )?;
        }
        if !filter_specs.is_empty() {
            check(
                unsafe {
                    (vtable.set_file_types)(
                        dialog.pointer(),
                        filter_specs.len() as u32,
                        filter_specs.as_ptr(),
                    )
                },
                "Set native dialog file types",
            )?;
        }
        if matches!(kind, DialogKind::SaveProject) {
            check(
                unsafe { (vtable.set_default_extension)(dialog.pointer(), extension.as_ptr()) },
                "Set project extension",
            )?;
        }
        // The OS pumps dialog events on this worker. The editor stays responsive
        // and serializes commands while this one native dialog is outstanding.
        let result = unsafe { (vtable.show)(dialog.pointer(), ptr::null_mut()) };
        if result == CANCELLED {
            return Ok(None);
        }
        check(result, "Show native dialog")?;
        selected_path(&dialog).map(Some)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Exercises the actual COM factories and hand-written vtable ABI without
        /// showing an interactive dialog, including a Unicode filesystem item.
        #[test]
        fn native_com_dialogs_accept_unicode_paths_without_showing_ui() {
            let _apartment = Apartment::initialize().unwrap();
            let current_directory = std::env::current_dir().unwrap();
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let folder = std::env::temp_dir().join(format!("ModuleEngine-dialog-Русский-{unique}"));
            std::fs::create_dir(&folder).unwrap();
            let shell = initial_folder(&folder).unwrap().unwrap();
            assert_eq!(shell_path(&shell).unwrap(), folder);
            for class in [&OPEN_DIALOG, &SAVE_DIALOG] {
                let mut dialog = ptr::null_mut();
                check(
                    unsafe {
                        CoCreateInstance(
                            class,
                            ptr::null_mut(),
                            INPROC_SERVER,
                            &FILE_DIALOG,
                            &mut dialog,
                        )
                    },
                    "Create native test dialog",
                )
                .unwrap();
                let dialog =
                    OwnedCom::take(dialog.cast::<FileDialog>(), "Create native test dialog")
                        .unwrap();
                let vtable = unsafe { &*(*dialog.pointer()).vtable };
                let mut options = 0;
                check(
                    unsafe { (vtable.get_options)(dialog.pointer(), &mut options) },
                    "Get native test options",
                )
                .unwrap();
                check(
                    unsafe {
                        (vtable.set_options)(
                            dialog.pointer(),
                            options | FORCE_FILESYSTEM | NO_CHANGE_DIR,
                        )
                    },
                    "Set native test options",
                )
                .unwrap();
                check(
                    unsafe { (vtable.set_folder)(dialog.pointer(), shell.pointer()) },
                    "Set Unicode native test folder",
                )
                .unwrap();
                let name = wide(OsStr::new("Моя игра.json")).unwrap();
                check(
                    unsafe { (vtable.set_file_name)(dialog.pointer(), name.as_ptr()) },
                    "Set Unicode native test filename",
                )
                .unwrap();
            }
            assert_eq!(std::env::current_dir().unwrap(), current_directory);
            drop(shell);
            std::fs::remove_dir(&folder).unwrap();
        }
    }
}
