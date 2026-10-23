//! Upstream's `about.py` + `ui/about.ui`.

use gtk::prelude::*;

pub fn show(parent: &gtk::Window) {
    let about = gtk::AboutDialog::builder()
        .transient_for(parent)
        .destroy_with_parent(true)
        .modal(true)
        .program_name("Virtual Machine Manager")
        .version(env!("CARGO_PKG_VERSION"))
        .copyright("Copyright (C) 2006-2020 Red Hat Inc.")
        .comments("Powered by libvirt")
        .website("https://virt-manager.org/")
        .website_label("https://virt-manager.org/")
        .authors([
            "Daniel P. Berrange <berrange@redhat.com>",
            "Cole Robinson <crobinso@redhat.com>",
            "Hugh O. Brock <hbrock@redhat.com>",
        ])
        .artists([
            "Máirín Duffy <duffy@redhat.com>",
            "Mike Langlie <mlanglie@redhat.com>",
            "Jeremy Perry <jeperry@redhat.com>",
            "Jakub Steiner <jsteiner@redhat.com>",
        ])
        .logo_icon_name("virt-manager")
        .license_type(gtk::License::Gpl20)
        .build();
    about.present();
}
