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

/// Exercise the same real COM creation/configuration used by all native picker
/// commands without opening a modal window. Only available to workflow tests.
#[cfg(all(test, windows))]
pub(crate) fn test_configure_initial_folder(initial_dir: &Path) -> Result<(), String> {
    platform::test_configure_initial_folder(initial_dir)
}

#[derive(Clone, Copy)]
enum DialogKind {
    OpenProject,
    SaveProject,
    Folder,
    ImportAsset,
}

#[cfg(all(test, not(windows)))]
mod unsupported_platform_tests {
    use super::*;

    #[test]
    fn all_public_pickers_report_windows_only_without_ui_or_io() {
        let path = Path::new("unused-dialog-fixture/project.json");
        for result in [
            open_project(path),
            save_project(path),
            pick_folder("Unused picker", path),
            import_asset(path),
        ] {
            assert_eq!(
                result.unwrap_err(),
                "Native file and folder dialogs are currently available only on Windows."
            );
        }
    }
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

    /// The Shell hint needs ordinary absolute Win32 spelling. This conversion
    /// never changes an IO path, selection, binding, or process current directory.
    fn ordinary_shell_path(path: &Path) -> Option<PathBuf> {
        use std::path::{Component, Prefix};
        let mut units: Vec<_> = path.as_os_str().encode_wide().collect();
        if units.contains(&0) {
            return None;
        }
        for unit in &mut units {
            if *unit == b'/' as u16 {
                *unit = b'\\' as u16;
            }
        }
        let normalized = PathBuf::from(OsString::from_wide(&units));
        let ordinary = match normalized.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(_, _) => {
                    // Names with significant verbatim-only trailing characters
                    // cannot safely be represented as ordinary Shell paths.
                    if normalized.components().any(|component| {
                        if let Component::Normal(name) = component {
                            let name: Vec<_> = name.encode_wide().collect();
                            name != [b'.' as u16]
                                && name != [b'.' as u16, b'.' as u16]
                                && matches!(name.last(), Some(32 | 46))
                        } else {
                            false
                        }
                    }) {
                        return None;
                    }
                    if let Prefix::VerbatimUNC(server, share) = prefix.kind() {
                        let mut ordinary = OsString::from("\\\\");
                        ordinary.push(server);
                        ordinary.push("\\");
                        ordinary.push(share);
                        let count = prefix.as_os_str().encode_wide().count();
                        ordinary.push(OsString::from_wide(&units[count..]));
                        ordinary
                    } else {
                        OsString::from_wide(&units[4..])
                    }
                }
                Prefix::DeviceNS(_) | Prefix::Verbatim(_) => return None,
                Prefix::Disk(_) | Prefix::UNC(_, _) => normalized.into_os_string(),
            },
            _ => normalized.into_os_string(),
        };
        Some(PathBuf::from(ordinary))
    }

    fn folder_hint_path(path: &Path) -> Option<(PathBuf, PathBuf)> {
        // Validate the namespace first, but canonicalize the ORIGINAL IO path.
        // This resolves relative/slash/dot spellings without silently changing
        // verbatim-only path semantics. Unrepresentable hints are simply absent.
        ordinary_shell_path(path)?;
        let canonical = path.canonicalize().ok()?;
        let ordinary = ordinary_shell_path(&canonical)?;
        if !ordinary.is_dir() || ordinary.canonicalize().ok()? != canonical {
            return None;
        }
        Some((ordinary, canonical))
    }

    fn initial_folder(path: &Path) -> Result<Option<OwnedCom<ShellItem>>, String> {
        // Initial location is optional. A missing directory or a Shell parser
        // failure must not prevent the user choosing another destination.
        let Some((absolute, canonical)) = folder_hint_path(path) else {
            return Ok(None);
        };
        let Ok(encoded) = wide(absolute.as_os_str()) else {
            return Ok(None);
        };
        let mut item = ptr::null_mut();
        let result = unsafe {
            SHCreateItemFromParsingName(encoded.as_ptr(), ptr::null_mut(), &SHELL_ITEM, &mut item)
        };
        if result < 0 {
            return Ok(None);
        }
        let Some(item) = NonNull::new(item.cast::<ShellItem>()).map(OwnedCom) else {
            return Ok(None);
        };
        if shell_path(&item)
            .ok()
            .and_then(|path| path.canonicalize().ok())
            .as_ref()
            != Some(&canonical)
        {
            return Ok(None);
        }
        Ok(Some(item))
    }

    fn apply_initial_folder(dialog: &OwnedCom<FileDialog>, path: &Path) {
        if let Ok(Some(folder)) = initial_folder(path) {
            let vtable = unsafe { &*(*dialog.pointer()).vtable };
            // SetFolder is another optional hint. Even a valid shell item may
            // be unavailable to this particular dialog or its current policy.
            let _ = unsafe { (vtable.set_folder)(dialog.pointer(), folder.pointer()) };
        }
    }

    fn show_configured_dialog(dialog: &OwnedCom<FileDialog>) -> Result<Option<PathBuf>, String> {
        let vtable = unsafe { &*(*dialog.pointer()).vtable };
        let result = unsafe { (vtable.show)(dialog.pointer(), ptr::null_mut()) };
        if result == CANCELLED {
            return Ok(None);
        }
        check(result, "Show native dialog")?;
        selected_path(dialog).map(Some)
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
        with_configured_dialog(kind, initial_dir, file_name, title, show_configured_dialog)
    }

    fn with_configured_dialog<T>(
        kind: DialogKind,
        initial_dir: &Path,
        file_name: Option<&OsStr>,
        title: &str,
        run: impl FnOnce(&OwnedCom<FileDialog>) -> Result<T, String>,
    ) -> Result<T, String> {
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
        configure_dialog(
            &dialog,
            kind,
            initial_dir,
            file_name.as_deref(),
            &title,
            &filter_specs,
            &extension,
        )?;
        run(&dialog)
    }

    fn configure_dialog(
        dialog: &OwnedCom<FileDialog>,
        kind: DialogKind,
        initial_dir: &Path,
        file_name: Option<&[u16]>,
        title: &[u16],
        filter_specs: &[FilterSpec],
        extension: &[u16],
    ) -> Result<(), String> {
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
        apply_initial_folder(dialog, initial_dir);
        if let Some(file_name) = file_name {
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
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn test_configure_initial_folder(initial_dir: &Path) -> Result<(), String> {
        for kind in [
            DialogKind::OpenProject,
            DialogKind::SaveProject,
            DialogKind::Folder,
            DialogKind::ImportAsset,
        ] {
            let file_name = match kind {
                DialogKind::SaveProject => Some(OsStr::new("Моя игра.json")),
                DialogKind::OpenProject | DialogKind::Folder | DialogKind::ImportAsset => None,
            };
            with_configured_dialog(
                kind,
                initial_dir,
                file_name,
                "Dialog configuration check",
                |_| Ok(()),
            )?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{cell::RefCell, rc::Rc};

        const TEST_FAILURE: Hresult = 0x8000_4005_u32 as i32;
        type Calls = Rc<RefCell<Vec<&'static str>>>;

        // An injected COM object exercises the production configuration and
        // Show/result flow without opening a window. Its Show outcome is not
        // evidence of an interactive native selection or cancellation.
        #[repr(C)]
        struct FakeDialog {
            interface: FileDialog,
            calls: Calls,
            reject: Option<&'static str>,
            show_result: Hresult,
            selection: Option<OwnedCom<ShellItem>>,
        }

        unsafe fn fake_call(dialog: *mut FileDialog, name: &'static str) -> Hresult {
            let dialog = unsafe { &mut *dialog.cast::<FakeDialog>() };
            dialog.calls.borrow_mut().push(name);
            if dialog.reject == Some(name) {
                TEST_FAILURE
            } else {
                0
            }
        }

        unsafe extern "system" fn fake_release(unknown: *mut Unknown) -> u32 {
            let dialog = unsafe { Box::from_raw(unknown.cast::<FakeDialog>()) };
            dialog.calls.borrow_mut().push("release");
            drop(dialog);
            0
        }

        unsafe extern "system" fn fake_show(
            dialog: *mut FileDialog,
            _owner: *mut c_void,
        ) -> Hresult {
            unsafe { fake_call(dialog, "show") };
            unsafe { (*dialog.cast::<FakeDialog>()).show_result }
        }

        unsafe extern "system" fn fake_get_options(
            dialog: *mut FileDialog,
            options: *mut u32,
        ) -> Hresult {
            let result = unsafe { fake_call(dialog, "get_options") };
            unsafe { *options = 0 };
            result
        }

        unsafe extern "system" fn fake_set_options(
            dialog: *mut FileDialog,
            _options: u32,
        ) -> Hresult {
            unsafe { fake_call(dialog, "set_options") }
        }

        unsafe extern "system" fn fake_set_folder(
            dialog: *mut FileDialog,
            _folder: *mut ShellItem,
        ) -> Hresult {
            unsafe { fake_call(dialog, "set_folder") };
            0x8007_0057_u32 as i32 // A valid initial hint can still be rejected.
        }

        unsafe extern "system" fn fake_set_title(
            dialog: *mut FileDialog,
            _title: *const u16,
        ) -> Hresult {
            unsafe { fake_call(dialog, "set_title") }
        }

        unsafe extern "system" fn fake_set_file_name(
            dialog: *mut FileDialog,
            _file_name: *const u16,
        ) -> Hresult {
            unsafe { fake_call(dialog, "set_file_name") }
        }

        unsafe extern "system" fn fake_set_file_types(
            dialog: *mut FileDialog,
            _count: u32,
            _filters: *const FilterSpec,
        ) -> Hresult {
            unsafe { fake_call(dialog, "set_file_types") }
        }

        unsafe extern "system" fn fake_set_extension(
            dialog: *mut FileDialog,
            _extension: *const u16,
        ) -> Hresult {
            unsafe { fake_call(dialog, "set_default_extension") }
        }

        unsafe extern "system" fn fake_get_result(
            dialog: *mut FileDialog,
            result: *mut *mut ShellItem,
        ) -> Hresult {
            let status = unsafe { fake_call(dialog, "get_result") };
            unsafe { *result = ptr::null_mut() };
            if status < 0 {
                return status;
            }
            let dialog = unsafe { &mut *dialog.cast::<FakeDialog>() };
            if let Some(item) = dialog.selection.take() {
                unsafe { *result = item.pointer() };
                // GetResult transfers this reference to production selected_path.
                std::mem::forget(item);
            }
            0
        }

        static FAKE_VTABLE: FileDialogVtable = FileDialogVtable {
            unknown: UnknownVtable {
                query_interface: 0,
                add_ref: 0,
                release: fake_release,
            },
            show: fake_show,
            set_file_types: fake_set_file_types,
            set_file_type_index: 0,
            get_file_type_index: 0,
            advise: 0,
            unadvise: 0,
            set_options: fake_set_options,
            get_options: fake_get_options,
            set_default_folder: 0,
            set_folder: fake_set_folder,
            get_folder: 0,
            get_current_selection: 0,
            set_file_name: fake_set_file_name,
            get_file_name: 0,
            set_title: fake_set_title,
            set_ok_button_label: 0,
            set_file_name_label: 0,
            get_result: fake_get_result,
            add_place: 0,
            set_default_extension: fake_set_extension,
            close: 0,
            set_client_guid: 0,
            clear_client_data: 0,
            set_filter: 0,
        };

        fn fake_dialog(
            show_result: Hresult,
            reject: Option<&'static str>,
            selection: Option<OwnedCom<ShellItem>>,
        ) -> (OwnedCom<FileDialog>, Calls) {
            let calls = Rc::default();
            let dialog = Box::new(FakeDialog {
                interface: FileDialog {
                    vtable: &FAKE_VTABLE,
                },
                calls: Rc::clone(&calls),
                reject,
                show_result,
                selection,
            });
            (
                OwnedCom::take(Box::into_raw(dialog).cast(), "Create injected dialog").unwrap(),
                calls,
            )
        }

        fn configure_and_show_fake(
            dialog: &OwnedCom<FileDialog>,
            initial_dir: &Path,
        ) -> Result<Option<PathBuf>, String> {
            let name = wide(OsStr::new("Моя игра.json"))?;
            let title = wide(OsStr::new("Save Project As"))?;
            let extension = wide(OsStr::new("json"))?;
            let pattern = wide(OsStr::new("*.json"))?;
            let filter = [FilterSpec {
                name: title.as_ptr(),
                pattern: pattern.as_ptr(),
            }];
            // These are the same production boundaries and order used after
            // with_configured_dialog creates the actual COM object.
            configure_dialog(
                dialog,
                DialogKind::SaveProject,
                initial_dir,
                Some(&name),
                &title,
                &filter,
                &extension,
            )?;
            show_configured_dialog(dialog)
        }

        #[test]
        fn rejected_folder_hint_does_not_mask_cancel_show_or_result_outcomes() {
            let _apartment = Apartment::initialize().unwrap();
            let folder = std::env::current_dir().unwrap();
            for (show_result, reject, expected_error) in [
                (CANCELLED, None, None),
                (TEST_FAILURE, None, Some("Show native dialog failed")),
                (0, Some("get_result"), Some("Read selected item failed")),
                (0, None, Some("Read selected item returned no COM object")),
            ] {
                let (dialog, calls) = fake_dialog(show_result, reject, None);
                let result = configure_and_show_fake(&dialog, &folder);
                match expected_error {
                    Some(message) => assert!(result.unwrap_err().contains(message)),
                    None => assert!(result.unwrap().is_none()),
                }
                let calls = calls.borrow();
                assert!(calls.contains(&"set_folder"), "{calls:?}");
                assert!(calls.contains(&"show"), "{calls:?}");
                assert_eq!(calls.contains(&"get_result"), show_result == 0, "{calls:?}");
            }
        }

        #[test]
        fn required_configuration_errors_still_prevent_show() {
            let _apartment = Apartment::initialize().unwrap();
            let folder = std::env::current_dir().unwrap();
            for (operation, expected_error) in [
                ("get_options", "Read native dialog options failed"),
                ("set_options", "Set native dialog options failed"),
                ("set_title", "Set native dialog title failed"),
                ("set_file_name", "Set initial dialog filename failed"),
                ("set_file_types", "Set native dialog file types failed"),
                ("set_default_extension", "Set project extension failed"),
            ] {
                let (dialog, calls) = fake_dialog(CANCELLED, Some(operation), None);
                assert!(
                    configure_and_show_fake(&dialog, &folder)
                        .unwrap_err()
                        .contains(expected_error),
                    "{operation}"
                );
                assert!(!calls.borrow().contains(&"show"), "{operation}");
            }
        }

        #[test]
        fn injected_get_result_transfers_real_unicode_shell_item_to_selected_path() {
            let _apartment = Apartment::initialize().unwrap();
            let fixture = module_engine_script::TestArtifactGuard::new(
                &std::env::temp_dir(),
                "dialog-result",
            )
            .unwrap();
            let folder = fixture.path().join("Результат-😀");
            std::fs::create_dir(&folder).unwrap();
            let item = initial_folder(&folder).unwrap().unwrap();
            let (dialog, calls) = fake_dialog(0, None, Some(item));
            let selected = configure_and_show_fake(&dialog, &folder).unwrap().unwrap();
            assert_eq!(
                selected.canonicalize().unwrap(),
                folder.canonicalize().unwrap()
            );
            assert!(calls.borrow().contains(&"get_result"));
            // Show/GetResult are injected here; only the returned ShellItem and
            // production selected_path decoding use the actual Windows Shell.
        }

        #[test]
        fn shell_hint_spelling_preserves_utf16_and_only_maps_drive_or_unc_namespaces() {
            for (input, expected) in [
                (r"E:/Игры/assets", r"E:\Игры\assets"),
                (r"\\?\E:\Игры\assets", r"E:\Игры\assets"),
                (r"\\server\share\Игры", r"\\server\share\Игры"),
                (r"\\?\UNC\server\share\Игры", r"\\server\share\Игры"),
            ] {
                assert_eq!(
                    ordinary_shell_path(Path::new(input)),
                    Some(PathBuf::from(expected)),
                    "{input}"
                );
            }
            let mut units: Vec<_> = r"\\?\E:\folder-".encode_utf16().collect();
            units.push(0xd800); // Unpaired UTF-16 is valid in an OsString.
            let input = PathBuf::from(OsString::from_wide(&units));
            assert_eq!(
                ordinary_shell_path(&input)
                    .unwrap()
                    .as_os_str()
                    .encode_wide()
                    .collect::<Vec<_>>(),
                units[4..]
            );
            for path in [
                r"\\.\C:\folder",
                r"\\?\Volume{00000000-0000-0000-0000-000000000000}\folder",
                r"\\?\GLOBALROOT\Device\HarddiskVolume1\folder",
                r"\\?\E:\folder.\child",
                r"\\?\E:\folder \child",
                r"\\?\UNC\server\share\folder.",
            ] {
                assert!(ordinary_shell_path(Path::new(path)).is_none(), "{path}");
            }
            let null = PathBuf::from(OsString::from_wide(&[b'E' as u16, 0, b'x' as u16]));
            assert!(ordinary_shell_path(&null).is_none());
        }

        #[test]
        fn missing_and_unrepresentable_folder_hints_fall_back_without_network_io() {
            let _apartment = Apartment::initialize().unwrap();
            let fixture = module_engine_script::TestArtifactGuard::new(
                &std::env::temp_dir(),
                "dialog-absent",
            )
            .unwrap();
            let missing = fixture.path().join("missing");
            assert!(!missing.exists());
            assert!(initial_folder(&missing).unwrap().is_none());
            // These namespaces are rejected before filesystem or Shell lookup.
            // This proves lexical UNC fallback, not access to a real network share.
            for path in [
                r"\\.\GLOBALROOT\Device\missing",
                r"\\?\Volume{00000000-0000-0000-0000-000000000000}\",
                r"\\?\UNC\not-contacted\share\folder. ",
            ] {
                assert!(initial_folder(Path::new(path)).unwrap().is_none(), "{path}");
            }
            test_configure_initial_folder(&missing).unwrap();
        }

        #[test]
        fn long_existing_folder_hint_has_identity_or_safe_fallback_without_ui() {
            let _apartment = Apartment::initialize().unwrap();
            let fixture =
                module_engine_script::TestArtifactGuard::new(&std::env::temp_dir(), "dialog-long")
                    .unwrap();
            let mut folder = fixture.path().canonicalize().unwrap();
            while folder.as_os_str().encode_wide().count() < 320 {
                folder.push("long-Unicode-Русский-directory-segment");
            }
            std::fs::create_dir_all(&folder).unwrap();
            let expected = folder.canonicalize().unwrap();
            match initial_folder(&folder).unwrap() {
                Some(shell) => {
                    assert_eq!(
                        shell_path(&shell).unwrap().canonicalize().unwrap(),
                        expected
                    );
                    eprintln!(
                        "long existing hint: ShellItem identity supported; SetFolder remains best-effort"
                    );
                }
                None => eprintln!(
                    "long existing hint: safe fallback; Shell long-path support not established"
                ),
            }
            test_configure_initial_folder(&folder).unwrap();
        }

        #[test]
        fn native_initial_folder_accepts_normalized_path_spellings_without_ui() {
            let _apartment = Apartment::initialize().unwrap();
            let cwd = std::env::current_dir().unwrap();
            let fixture =
                module_engine_script::TestArtifactGuard::new(&std::env::temp_dir(), "dialog-hints")
                    .unwrap();
            let folder = fixture.path().join("Русский-😀");
            std::fs::create_dir(&folder).unwrap();
            std::fs::create_dir(folder.join("child")).unwrap();
            let canonical = folder.canonicalize().unwrap();
            let slash = PathBuf::from(folder.to_str().unwrap().replace('\\', "/"));
            for (path, expected) in [
                (folder.clone(), canonical.clone()),
                (slash, canonical.clone()),
                (canonical.clone(), canonical.clone()),
                (folder.join("."), canonical.clone()),
                (folder.join("child").join(".."), canonical),
                (PathBuf::from("."), cwd.canonicalize().unwrap()),
                (PathBuf::from("./"), cwd.canonicalize().unwrap()),
            ] {
                let shell = initial_folder(&path)
                    .unwrap()
                    .unwrap_or_else(|| panic!("Missing folder hint: {path:?}"));
                assert_eq!(
                    shell_path(&shell).unwrap().canonicalize().unwrap(),
                    expected,
                    "{path:?}"
                );
            }
            assert_eq!(std::env::current_dir().unwrap(), cwd);
        }

        /// Exercises the actual COM factories and hand-written vtable ABI without
        /// showing an interactive dialog, including a Unicode filesystem item.
        #[test]
        fn native_com_dialogs_accept_unicode_paths_without_showing_ui() {
            let _apartment = Apartment::initialize().unwrap();
            let current_directory = std::env::current_dir().unwrap();
            let fixture =
                module_engine_script::TestArtifactGuard::new(&std::env::temp_dir(), "dialog-com")
                    .unwrap();
            let folder = fixture.path().join("Русский-😀");
            std::fs::create_dir(&folder).unwrap();
            let shell = initial_folder(&folder).unwrap().unwrap();
            assert_eq!(
                shell_path(&shell).unwrap().canonicalize().unwrap(),
                folder.canonicalize().unwrap()
            );
            // Every picker uses the same factory/configuration as production,
            // including its actual filename policy, but never calls Show.
            test_configure_initial_folder(&folder).unwrap();
            assert_eq!(std::env::current_dir().unwrap(), current_directory);
        }
    }
}
