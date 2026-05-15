%global crate_name facegate
%global broker     facegate-brokerd
%global pam_lib    pam_facegate.so

Name:           facegate
Version:        0.3.1
Release:        1%{?dist}
Summary:        Native facial authentication for Linux PAM
License:        GPL-3.0-or-later
URL:            https://github.com/me02329/facegate
Source0:        %{url}/releases/download/v%{version}/%{name}-%{version}.tar.gz

# COPR uses mock chroots that already have rust/cargo. EPEL needs the
# rust-toolset RPM. clang+pkgconf are required by some -sys crates.
BuildRequires:  rust >= 1.95
BuildRequires:  cargo
BuildRequires:  clang
BuildRequires:  pkgconf-pkg-config
BuildRequires:  systemd-rpm-macros

# Runtime: ORT shared lib, V4L2 utils for `facegate cameras`, systemd.
# Fedora/EL ship onnxruntime in EPEL/COPR; if a build target lacks it,
# the broker will refuse to start until the user installs it manually.
Requires:       onnxruntime
Requires:       v4l-utils
Requires(post): systemd
Requires(preun): systemd
Requires(postun): systemd

ExclusiveArch:  x86_64 aarch64

%description
Facegate is a native Rust facial-authentication stack for Linux PAM.
It plugs into sudo, login, and screen-lock via a small native module
(pam_facegate.so), captures frames over V4L2, and delegates matching
to a hardened system daemon (facegate-brokerd) that owns the biometric
templates. SCRFD + ArcFace run via ONNX Runtime; templates are never
readable from the authenticating user's UID.

%prep
%autosetup -p1 -n %{name}-%{version}

%build
# Reproducible-ish: disable network fetches (vendored .cargo/config or
# the source tarball ships Cargo.lock — we rely on --locked).
%{cargo_build}

%install
install -Dm755 target/release/%{name}             %{buildroot}%{_bindir}/%{name}
install -Dm755 target/release/%{broker}           %{buildroot}%{_bindir}/%{broker}
install -Dm755 target/release/lib%{pam_lib:.so=}.so %{buildroot}%{_libdir}/security/%{pam_lib}

install -Dm644 config.example.toml                %{buildroot}%{_sysconfdir}/%{name}/config.toml
install -Dm644 systemd/%{broker}.service          %{buildroot}%{_unitdir}/%{broker}.service
install -Dm644 systemd/%{name}-watch.service      %{buildroot}%{_userunitdir}/%{name}-watch.service
install -Dm644 docs/%{name}.1                     %{buildroot}%{_mandir}/man1/%{name}.1

# Shell completions
install -d %{buildroot}%{_datadir}/bash-completion/completions
install -d %{buildroot}%{_datadir}/zsh/site-functions
install -d %{buildroot}%{_datadir}/fish/vendor_completions.d
target/release/%{name} completions bash > %{buildroot}%{_datadir}/bash-completion/completions/%{name}        2>/dev/null || :
target/release/%{name} completions zsh  > %{buildroot}%{_datadir}/zsh/site-functions/_%{name}                2>/dev/null || :
target/release/%{name} completions fish > %{buildroot}%{_datadir}/fish/vendor_completions.d/%{name}.fish     2>/dev/null || :

%pre
getent group %{name} >/dev/null || groupadd --system %{name}
getent passwd %{name} >/dev/null || \
    useradd --system --no-create-home --home-dir %{_sharedstatedir}/%{name} \
            --gid %{name} --shell /sbin/nologin %{name}
exit 0

%post
install -d -m 0755 -o root          -g root          %{_sysconfdir}/%{name}
install -d -m 0700 -o %{name}       -g %{name}       %{_sharedstatedir}/%{name}
if [ ! -e %{_sharedstatedir}/%{name}/audit.log ]; then
    install -m 0600 -o %{name} -g %{name} /dev/null %{_sharedstatedir}/%{name}/audit.log
fi
%systemd_post %{broker}.service

%preun
%systemd_preun %{broker}.service

%postun
%systemd_postun_with_restart %{broker}.service

%files
%license LICENSE
%doc README.md CHANGELOG.md SECURITY.md
%{_bindir}/%{name}
%{_bindir}/%{broker}
%{_libdir}/security/%{pam_lib}
%dir %{_sysconfdir}/%{name}
%config(noreplace) %{_sysconfdir}/%{name}/config.toml
%{_unitdir}/%{broker}.service
%{_userunitdir}/%{name}-watch.service
%{_mandir}/man1/%{name}.1*
%{_datadir}/bash-completion/completions/%{name}
%{_datadir}/zsh/site-functions/_%{name}
%{_datadir}/fish/vendor_completions.d/%{name}.fish

%changelog
* Fri May 15 2026 me02329 <github@martial.aleeas.com> - 0.3.1-1
- Distribution-only release. Provenance attestation, source tarball,
  CycloneDX SBOM. No runtime behaviour changes vs. 0.3.0.
* Thu May 14 2026 me02329 <github@martial.aleeas.com> - 0.3.0-1
- Initial COPR build.
