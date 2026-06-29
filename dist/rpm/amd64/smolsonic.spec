Name:           smolsonic
Version:        0.6.2
Release:        1%{?dist}
Summary:        A tiny Subsonic-compatible music server written in Rust

License:        MIT
URL:            https://github.com/tsirysndr/smolsonic

BuildArch:      x86_64

Requires: glibc

%description
smolsonic is a self-contained Subsonic-compatible music server. Point it at
a folder of music, give it a username and a password in a TOML file, and any
Subsonic client can browse and stream your library. Ships an optional
S3-compatible upload API and an embedded admin web UI.

%prep
# Prepare the build environment

%build
# Build steps (if any)

%install
mkdir -p %{buildroot}/usr/local/bin
mkdir -p %{buildroot}/usr/share/smolsonic
mkdir -p %{buildroot}/usr/lib/systemd/user
cp -r %{_sourcedir}/amd64/usr %{buildroot}/

%files
/usr/local/bin/smolsonic
/usr/share/smolsonic/smolsonic.example.toml
/usr/lib/systemd/user/smolsonic.service

%post
EXAMPLE_CFG=/usr/share/smolsonic/smolsonic.example.toml

if [ "$1" -eq 1 ]; then
    # Fresh install
    if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
        USER_HOME=$(getent passwd "$SUDO_USER" | cut -d: -f6)
        USER_UID=$(id -u "$SUDO_USER" 2>/dev/null)
        if [ -n "$USER_UID" ]; then
            sudo -u "$SUDO_USER" mkdir -p \
                "$USER_HOME/.config/smolsonic" \
                "$USER_HOME/.local/share/smolsonic" || :

            if [ ! -f "$USER_HOME/.config/smolsonic/smolsonic.toml" ] && [ -f "$EXAMPLE_CFG" ]; then
                sudo -u "$SUDO_USER" cp "$EXAMPLE_CFG" "$USER_HOME/.config/smolsonic/smolsonic.toml" || :
                echo "smolsonic: wrote example config to $USER_HOME/.config/smolsonic/smolsonic.toml"
            fi

            sudo -u "$SUDO_USER" XDG_RUNTIME_DIR=/run/user/$USER_UID \
                systemctl --user daemon-reload &> /dev/null || :

            echo "smolsonic: edit ~/.config/smolsonic/smolsonic.toml, then start with:"
            echo "  systemctl --user enable --now smolsonic.service"
        fi
    else
        systemctl daemon-reload &> /dev/null || :
        echo "smolsonic: each user can enable the service with:"
        echo "  systemctl --user enable --now smolsonic.service"
    fi
fi

%preun
if [ "$1" -eq 0 ]; then
    # Uninstall (not upgrade)
    if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
        USER_UID=$(id -u "$SUDO_USER" 2>/dev/null)
        if [ -n "$USER_UID" ]; then
            sudo -u "$SUDO_USER" XDG_RUNTIME_DIR=/run/user/$USER_UID systemctl --user stop smolsonic.service &> /dev/null || :
            sudo -u "$SUDO_USER" XDG_RUNTIME_DIR=/run/user/$USER_UID systemctl --user disable smolsonic.service &> /dev/null || :
        fi
    fi
fi

%postun
if [ "$1" -eq 0 ]; then
    # Uninstall (not upgrade)
    if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
        USER_UID=$(id -u "$SUDO_USER" 2>/dev/null)
        if [ -n "$USER_UID" ]; then
            sudo -u "$SUDO_USER" XDG_RUNTIME_DIR=/run/user/$USER_UID systemctl --user daemon-reload &> /dev/null || :
        fi
    fi
fi
