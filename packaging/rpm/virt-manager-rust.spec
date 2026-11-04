# Empacota a árvore pronta do packaging/install.sh (packaging/rpm/build.sh).
%global debug_package %{nil}

Name:           virt-manager-rust
Version:        %{pkgversion}
Release:        1%{?dist}
Summary:        Desktop tool for managing virtual machines (Rust port)
License:        GPL-2.0-or-later
URL:            https://github.com/VexFlint/Virt-manager-rust

Requires:       gsettings-desktop-schemas
Requires:       hicolor-icon-theme
Recommends:     libvirt-daemon-kvm
Recommends:     osinfo-db
Recommends:     xorriso
# Substitui o virt-manager Python: mesmos caminhos, mesmo schema.
Conflicts:      virt-manager virt-manager-common virt-install
Provides:       virt-manager = %{version}-%{release}
Provides:       virt-install = %{version}-%{release}

%description
Port of virt-manager 5.1.0 to Rust and GTK4, with the same screens and
behaviour. Includes the virt-manager GUI and the virt-install, virt-clone
and virt-xml command line tools. Replaces the virt-manager and
virt-install packages: same paths, same GSettings schema.

%install
cp -a %{_sourcedir}/pkg/. %{buildroot}/
install -D -m 0644 %{_sourcedir}/LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE
install -D -m 0644 %{_sourcedir}/CHANGELOG.md %{buildroot}%{_docdir}/%{name}/CHANGELOG.md

%files
%license %{_licensedir}/%{name}/LICENSE
%doc %{_docdir}/%{name}/CHANGELOG.md
%{_bindir}/virt-manager
%{_bindir}/virt-install
%{_bindir}/virt-clone
%{_bindir}/virt-xml
%{_datadir}/applications/virt-manager.desktop
%{_datadir}/metainfo/virt-manager.appdata.xml
%{_datadir}/glib-2.0/schemas/org.virt-manager.virt-manager.gschema.xml
%{_datadir}/icons/hicolor/*/apps/virt-manager.png
%{_mandir}/man1/virt-*.1*
%{_datadir}/bash-completion/completions/virt-*
%{_datadir}/zsh/site-functions/_virt-*
%{_datadir}/fish/vendor_completions.d/virt-*.fish
