//! Upstream's `virtManager/vmmenu.py`: the VM action menu (context menu
//! on a VM row), the Shut Down submenu (also the toolbar's dropdown),
//! and `VMActionUI` — confirmation prompts plus the async libvirt call
//! for each lifecycle action.
//!
//! GTK4 menus are `GMenuModel`s over actions, so the menu items map to
//! actions in a `vm` action group, enabled/disabled by
//! [`update_widget_states`] from the same state map upstream uses.
//! Clone, Migrate and Delete have their actions but stay disabled
//! until their windows are ported.

use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::domain::{DomainAction, VmmDomain};
use crate::engine;
use crate::error;

pub const GROUP: &str = "vm";

/// Not-yet-ported windows: their actions exist but never enable.
const UNPORTED: &[&str] = &["clone"];

/// `VMShutdownMenu`.
pub fn shutdown_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let main = gio::Menu::new();
    main.append(Some("_Reboot"), Some("vm.reboot"));
    main.append(Some("_Shut Down"), Some("vm.shutdown"));
    main.append(Some("F_orce Reset"), Some("vm.reset"));
    main.append(Some("_Force Off"), Some("vm.destroy"));
    menu.append_section(None, &main);
    let save = gio::Menu::new();
    save.append(Some("Sa_ve"), Some("vm.save"));
    menu.append_section(None, &save);
    menu
}

/// `VMActionMenu`, built per popup: upstream toggles Pause/Resume
/// visibility and swaps Run's label to Restore, which a static
/// `GMenuModel` can't do.
pub fn action_menu(vm: Option<&VmmDomain>, show_open: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    let power = gio::Menu::new();
    let can_restore = vm.is_some_and(|vm| vm.has_managed_save());
    power.append(
        Some(if can_restore { "_Restore" } else { "_Run" }),
        Some("vm.run"),
    );
    if vm.is_some_and(|vm| vm.is_paused()) {
        power.append(Some("R_esume"), Some("vm.resume"));
    } else {
        power.append(Some("_Pause"), Some("vm.suspend"));
    }
    power.append_submenu(Some("_Shut Down"), &shutdown_menu());
    menu.append_section(None, &power);

    let manage = gio::Menu::new();
    manage.append(Some("Clone..."), Some("vm.clone"));
    manage.append(Some("Migrate..."), Some("vm.migrate"));
    manage.append(Some("_Delete"), Some("vm.delete"));
    menu.append_section(None, &manage);

    if show_open {
        let open = gio::Menu::new();
        open.append(Some("_Open"), Some("vm.show"));
        menu.append_section(None, &open);
    }
    menu
}

/// Creates the `vm` action group. `current_vm` is the owning window's
/// selection; `parent` is the window dialogs attach to.
pub fn create_actions(
    parent: &gtk::Window,
    current_vm: Rc<dyn Fn() -> Option<VmmDomain>>,
) -> gio::SimpleActionGroup {
    let group = gio::SimpleActionGroup::new();
    let names = [
        "run", "suspend", "resume", "reboot", "shutdown", "reset", "destroy", "save", "clone",
        "migrate", "delete", "show",
    ];
    for name in names {
        let action = gio::SimpleAction::new(name, None);
        let parent = parent.downgrade();
        let current_vm = current_vm.clone();
        action.connect_activate(move |_, _| {
            let (Some(parent), Some(vm)) = (parent.upgrade(), current_vm()) else {
                return;
            };
            glib::spawn_future_local(async move {
                match name {
                    "run" => run(&parent, &vm).await,
                    "suspend" => suspend(&parent, &vm).await,
                    "resume" => resume(&parent, &vm).await,
                    "reboot" => reboot(&parent, &vm).await,
                    "shutdown" => shutdown(&parent, &vm).await,
                    "reset" => reset(&parent, &vm).await,
                    "destroy" => destroy(&parent, &vm).await,
                    "save" => save(&parent, &vm).await,
                    "show" => crate::vmwindow::VmmVMWindow::show_instance(&vm),
                    "migrate" => crate::migrate::VmmMigrateDialog::show_instance(&parent, &vm),
                    "delete" => crate::delete::DeleteDialog::show(
                        Some(&parent),
                        &vm,
                        crate::delete::DeleteKind::Vm,
                    ),
                    _ => {}
                }
            });
        });
        group.add_action(&action);
    }
    update_widget_states(&group, None);
    group
}

/// The union of `VMShutdownMenu`'s and `VMActionMenu`'s state maps.
pub fn update_widget_states(group: &gio::SimpleActionGroup, vm: Option<&VmmDomain>) {
    let st = |f: fn(&VmmDomain) -> bool| vm.is_some_and(f);
    let statemap = [
        ("run", st(VmmDomain::is_runable)),
        ("suspend", st(VmmDomain::is_stoppable)),
        ("resume", st(VmmDomain::is_paused)),
        ("reboot", st(VmmDomain::is_stoppable)),
        ("shutdown", st(VmmDomain::is_stoppable)),
        ("reset", st(VmmDomain::is_stoppable)),
        ("destroy", st(VmmDomain::is_destroyable)),
        ("save", st(VmmDomain::is_destroyable)),
        ("show", vm.is_some()),
        ("migrate", st(VmmDomain::is_stoppable)),
        ("delete", vm.is_some()),
    ];
    for (name, enabled) in statemap {
        set_enabled(group, name, enabled);
    }
    for name in UNPORTED {
        set_enabled(group, name, false);
    }
}

fn set_enabled(group: &gio::SimpleActionGroup, name: &str, enabled: bool) {
    if let Some(action) = group
        .lookup_action(name)
        .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
    {
        action.set_enabled(enabled);
    }
}

/// `vmmAsyncJob.simple_async_noshow`: run the call, show an error
/// dialog prefixed with `errorintro` if it fails.
async fn simple_async_noshow(
    parent: &gtk::Window,
    vm: &VmmDomain,
    action: DomainAction,
    errorintro: &str,
) {
    if let Err(e) = vm.run_action(action).await {
        error::show_err(Some(parent), &format!("{errorintro}: {e}"), Some(&e), "").await;
    }
}

pub async fn run(parent: &gtk::Window, vm: &VmmDomain) {
    if !vm.has_managed_save() {
        simple_async_noshow(parent, vm, DomainAction::Startup, "Error starting domain").await;
        return;
    }
    // Restoring a saved image: offer a regular boot if restore fails.
    let Err(e) = vm.run_action(DomainAction::Startup).await else {
        return;
    };
    let (res, _) = error::run_dialog(
        Some(parent),
        error::DialogSpec {
            kind: error::MessageType::Warning,
            buttons: error::Buttons::YesNo,
            text1: &format!("Error restoring domain: {e}"),
            text2: Some(
                "The domain could not be restored. Would you like\n\
                 to remove the saved state and perform a regular\n\
                 start up?",
            ),
            title: "",
            details: Some(&e),
            chktext: None,
        },
    )
    .await;
    if !res {
        return;
    }
    if let Err(e) = vm.run_action(DomainAction::RemoveSavedImage).await {
        error::show_err(
            Some(parent),
            &format!("Error removing domain state: {e}"),
            None,
            "",
        )
        .await;
        return;
    }
    simple_async_noshow(parent, vm, DomainAction::Startup, "Error starting domain").await;
}

pub async fn suspend(parent: &gtk::Window, vm: &VmmDomain) {
    let config = engine::get().config();
    let ok = error::chkbox_helper(
        Some(parent),
        || config.get_confirm_pause(),
        |v| config.set_confirm_pause(v),
        &format!("Are you sure you want to pause '{}'?", vm.get_name()),
        None,
    )
    .await;
    if ok {
        simple_async_noshow(parent, vm, DomainAction::Suspend, "Error pausing domain").await;
    }
}

pub async fn resume(parent: &gtk::Window, vm: &VmmDomain) {
    simple_async_noshow(parent, vm, DomainAction::Resume, "Error unpausing domain").await;
}

async fn confirm_poweroff(parent: &gtk::Window, text1: String) -> bool {
    let config = engine::get().config();
    error::chkbox_helper(
        Some(parent),
        || config.get_confirm_poweroff(),
        |v| config.set_confirm_poweroff(v),
        &text1,
        None,
    )
    .await
}

async fn confirm_forcepoweroff(parent: &gtk::Window, text1: String, text2: &str) -> bool {
    let config = engine::get().config();
    error::chkbox_helper(
        Some(parent),
        || config.get_confirm_forcepoweroff(),
        |v| config.set_confirm_forcepoweroff(v),
        &text1,
        Some(text2),
    )
    .await
}

pub async fn shutdown(parent: &gtk::Window, vm: &VmmDomain) {
    let text = format!("Are you sure you want to poweroff '{}'?", vm.get_name());
    if confirm_poweroff(parent, text).await {
        simple_async_noshow(
            parent,
            vm,
            DomainAction::Shutdown,
            "Error shutting down domain",
        )
        .await;
    }
}

pub async fn reboot(parent: &gtk::Window, vm: &VmmDomain) {
    let text = format!("Are you sure you want to reboot '{}'?", vm.get_name());
    if confirm_poweroff(parent, text).await {
        simple_async_noshow(parent, vm, DomainAction::Reboot, "Error rebooting domain").await;
    }
}

pub async fn reset(parent: &gtk::Window, vm: &VmmDomain) {
    let text = format!("Are you sure you want to force reset '{}'?", vm.get_name());
    let text2 = "This will immediately reset the VM without \
                 shutting down the OS and may cause data loss.";
    if confirm_forcepoweroff(parent, text, text2).await {
        simple_async_noshow(parent, vm, DomainAction::Reset, "Error resetting domain").await;
    }
}

pub async fn destroy(parent: &gtk::Window, vm: &VmmDomain) {
    let text = format!(
        "Are you sure you want to force poweroff '{}'?",
        vm.get_name()
    );
    let text2 = "This will immediately poweroff the VM without \
                 shutting down the OS and may cause data loss.";
    if confirm_forcepoweroff(parent, text, text2).await {
        simple_async_noshow(
            parent,
            vm,
            DomainAction::Destroy,
            "Error shutting down domain",
        )
        .await;
    }
}

/// Upstream shows a cancellable progress window ("Saving Virtual
/// Machine") via `vmmAsyncJob`; that window arrives with the asyncjob
/// port, until then the save runs like the other actions.
pub async fn save(parent: &gtk::Window, vm: &VmmDomain) {
    let text = format!("Are you sure you want to save '{}'?", vm.get_name());
    if confirm_poweroff(parent, text).await {
        simple_async_noshow(parent, vm, DomainAction::Save, "Error saving domain").await;
    }
}
