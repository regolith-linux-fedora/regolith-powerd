# Source1 is not in the repository. ext-rpm.sh makes it on the runner while it
# builds the SRPM, with `cargo vendor --locked` against Cargo.lock, so COPR
# builds offline from the SRPM.

# %%cargo_build runs cargo with RUSTC_BOOTSTRAP=1. The build script of the
# vendored rustix 0.37 (through zbus 3) then finds nightly features available
# and turns on rustc_attrs, whose attributes current rustc no longer has, and
# the build fails. Allowing no unstable features makes such probes fail, so
# crates take their stable code paths.
%global build_rustflags %{build_rustflags} -Zallow-features=

Name:           regolith-powerd
Version:        0.7.0
Release:        1%{?dist}
Summary:        Daemon that applies GNOME power settings to the sway session
# regolith-powerd is GPL-3.0-or-later (Cargo.toml says GPL-3.0+,
# debian/copyright). The rest covers the vendored crates: update it from the
# %%{cargo_license_summary} output in the build log whenever Cargo.lock
# changes.
License:        GPL-3.0-or-later AND BSD-3-Clause AND MIT AND (Apache-2.0 OR BSL-1.0) AND (Apache-2.0 OR MIT) AND (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND (Unlicense OR MIT)
URL:            https://github.com/regolith-linux/regolith-powerd
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo-rpm-macros >= 24
BuildRequires:  systemd-rpm-macros
# The gio and glib crates link against the system libraries.
BuildRequires:  pkgconfig(gio-2.0)

# The daemon reads org.gnome.settings-daemon.plugins.power, which Fedora
# ships only in gnome-settings-daemon (Debian has gnome-settings-daemon-common),
# and org.gnome.desktop.session. The copies in the repository are only read at
# build time.
Requires:       gnome-settings-daemon
Requires:       gsettings-desktop-schemas
# Commands in the swayidle script it generates: swayidle runs it, swaymsg and
# swaynag come with sway (or sway-regolith, which provides sway),
# gnome-session-quit with gnome-session, systemctl and poweroff with systemd.
Requires:       swayidle
Requires:       sway
Requires:       gnome-session
Requires:       systemd
Requires:       playerctl
# trawlcat looks up the lock command and wallpaper. Debian names trawldb.
Requires:       trawlcat
# The idle dim uses brightnessctl, which goes through logind, instead of the
# setuid light that Debian depends on (Source Change). powermgmt-base has no
# Fedora equivalent and isn't needed: the power source is read from sysfs
# (Source Change).
Requires:       brightnessctl
# The default lock command. Fedora 43 has no gtklock; 02-14 settles the lock
# screen.
Recommends:     gtklock

%description
A daemon for the sway session that turns the GNOME power settings (screen
dimming, blanking, locking, automatic suspend and the power button action)
into an idle daemon configuration for sway, and applies them again when they
change.

%prep
%autosetup -a1
%cargo_prep -v vendor

%build
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%{cargo_vendor_manifest}

%install
%cargo_install
install -Dpm 0644 data/regolith-init-powerd.service %{buildroot}%{_userunitdir}/regolith-init-powerd.service
# Debian's postinst (dh_installsystemduser) runs `deb-systemd-helper --user
# enable regolith-init-powerd.service`, which links it into
# regolith-wayland.target.wants for every user. A user preset does the same
# through %%systemd_user_post. data/*_ignore_*.conf are not installed, as in
# Debian: the daemon takes a logind inhibitor for the power key and the lid
# switch instead.
install -d %{buildroot}%{_userpresetdir}
echo "enable regolith-init-powerd.service" > %{buildroot}%{_userpresetdir}/90-regolith-powerd.preset

%check
%cargo_test

%post
%systemd_user_post regolith-init-powerd.service

%preun
%systemd_user_preun regolith-init-powerd.service

%postun
%systemd_user_postun regolith-init-powerd.service

%files
%license LICENSE.dependencies cargo-vendor.txt
%{_bindir}/regolith-powerd
%{_userunitdir}/regolith-init-powerd.service
%{_userpresetdir}/90-regolith-powerd.preset

%changelog
* Fri Nov 13 2026 Regolith Fedora Port <port@regolith.invalid> - 0.7.0-1
- Initial package
