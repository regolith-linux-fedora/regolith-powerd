use gio::{prelude::*, Application, ApplicationFlags};
use glib::g_log;
use gsettings_macro::gen_settings;
use std::{
    error::Error,
    mem::ManuallyDrop,
    process::{Child, Command},
};
use swayipc;

mod logind;

struct Manager {
    power_settings: PowerSettings,
    session_settings: SessionSettings,
}

#[gen_settings(
    file = "org.gnome.settings-daemon.plugins.power.gschema.xml",
    id = "org.gnome.settings-daemon.plugins.power"
)]
pub struct PowerSettings;

#[gen_settings(
    file = "org.gnome.desktop.session.gschema.xml",
    id = "org.gnome.desktop.session"
)]
pub struct SessionSettings;

impl Manager {
    pub fn new() -> Self {
        Self {
            session_settings: SessionSettings::new(),
            power_settings: PowerSettings::new(),
        }
    }
    pub fn run(self) -> Result<(), Box<dyn Error>> {
        let (send_reload_event, recv_reload_event) = async_channel::bounded(1);

        // Send initial Reload
        send_reload_event
            .send_blocking(())
            .expect("Failed to send reload");
        self.power_settings.handle_power_btn_action_change()?;

        let send_reload_cb = || {
            let tx_cpy = send_reload_event.clone();
            return move |_: &PowerSettings| {
                tx_cpy.send_blocking(()).expect("Cannot reload config");
            };
        };

        self.power_settings
            .connect_idle_brightness_changed(send_reload_cb());
        self.power_settings
            .connect_idle_dim_changed(send_reload_cb());
        self.power_settings
            .connect_sleep_inactive_ac_timeout_changed(send_reload_cb());
        self.power_settings
            .connect_sleep_inactive_ac_type_changed(send_reload_cb());
        self.power_settings
            .connect_sleep_inactive_battery_timeout_changed(send_reload_cb());
        self.power_settings
            .connect_sleep_inactive_battery_type_changed(send_reload_cb());
        self.power_settings
            .connect_power_button_action_changed(|s| {
                s.handle_power_btn_action_change()
                    .expect("Failed to set Power button action")
            });
        self.session_settings.connect_idle_delay_changed(move |_| {
            send_reload_event
                .clone()
                .send_blocking(())
                .expect("Failed to reload");
        });

        let mut sway_idle_child: Option<Child> = None;
        glib::spawn_future_local(async move {
            while let Ok(_) = recv_reload_event.recv().await {
                if let Some(prev_child) = sway_idle_child.as_mut() {
                    prev_child
                        .kill()
                        .unwrap_or_else(|e| println!("Cannot kill swayidle: {e}"));
                }
                let swayidle_args = self.get_swayidle_args();
                let child = Command::new("swayidle")
                    .arg("-w")
                    .args(swayidle_args)
                    .spawn();
                sway_idle_child = child.ok();
                g_log!(glib::LogLevel::Info, "Swayidle Reloaded");
            }
        });

        Ok(())
    }

    fn get_swayidle_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let display_off = format!("swaymsg output '*' dpms off");
        let display_on = format!("swaymsg output '*' dpms on");

        let default_lock = format!(
            "gtklock -d --background $(trawlcat regolith.lockscreen.wallpaper.file /dev/null)"
        );
        let lock_screen = format!("$(trawlcat wm.program.lock_sway \"{default_lock}\")");

        if self.power_settings.idle_dim() {
            let idle_brightness = self.power_settings.idle_brightness();
            let action = dim_cmd(idle_brightness);

            // Dim screen to $idle_brightness when idle dim timeout hit
            let dim_timeout_sec = self.session_settings.idle_delay();
            let mut swayidle_dim_timeout_args = Self::get_timeout_cmd(dim_timeout_sec, &action);
            let mut swayidle_dim_resume_args = Self::get_resume_cmd(&undim_cmd());
            args.append(&mut swayidle_dim_timeout_args);
            args.append(&mut swayidle_dim_resume_args);

            // Display off at 5 seconds after dim timeout
            let display_off_timeout_sec = dim_timeout_sec + 5;
            let mut swayidle_display_off_timeout_args =
                Self::get_timeout_cmd(display_off_timeout_sec, &display_off);
            let mut swayidle_display_off_resume_args = Self::get_resume_cmd(&display_on);
            args.append(&mut swayidle_display_off_timeout_args);
            args.append(&mut swayidle_display_off_resume_args);

            // Locked 10 seconds after dim timeout / 5 seconds after display off
            let lock_timeout_sec = display_off_timeout_sec + 5;
            let mut swayidle_lock_timeout_args =
                Self::get_timeout_cmd(lock_timeout_sec, &lock_screen);
            args.append(&mut swayidle_lock_timeout_args);
        }

        let (idle_action_ac_opt, resume_action_ac_opt) = {
            use SleepInactiveAcType::*;
            match self.power_settings.sleep_inactive_ac_type() {
                Suspend => (Some("systemctl suspend"), None),
                Hibernate => (Some("systemctl hibernate"), None),
                Blank => (
                    Some("swaymsg output * dpms off"),
                    Some("swaymsg output * dpms on"),
                ),
                Shutdown => (Some("poweroff"), None),
                Logout => (Some("gnome-session-quit --no-prompt"), None),
                Interactive => todo!("Show some gui prompt to the user"),
                _ => (None, None),
            }
        };
        let (idle_action_bat_opt, resume_action_bat_opt) = {
            use SleepInactiveBatteryType::*;
            match self.power_settings.sleep_inactive_battery_type() {
                Suspend => (Some("systemctl suspend"), None),
                Hibernate => (Some("systemctl hibernate"), None),
                Blank => (
                    Some("swaymsg output * dpms off"),
                    Some("swaymsg output * dpms on"),
                ),
                Shutdown => (Some("poweroff"), None),
                Logout => (Some("gnome-session-quit --no-prompt"), None),
                Interactive => todo!("Show some gui prompt to the user"),
                _ => (None, None),
            }
        };

        if let Some(action) = idle_action_ac_opt {
            let timeout = self.power_settings.sleep_inactive_ac_timeout();
            let on_timout = power_source_guard(PowerSource::Ac, action);
            let mut swayidle_timeout_args = Self::get_timeout_cmd(timeout as u32, &on_timout);
            args.append(&mut swayidle_timeout_args);

            if let Some(resume_action) = resume_action_ac_opt {
                let mut swayidle_resume_args = Self::get_resume_cmd(resume_action);
                args.append(&mut swayidle_resume_args);
            }
        }

        if let Some(action) = idle_action_bat_opt {
            let timeout = self.power_settings.sleep_inactive_battery_timeout();
            let on_timout = power_source_guard(PowerSource::Battery, action);
            let mut swayidle_timout_args = Self::get_timeout_cmd(timeout as u32, &on_timout);
            args.append(&mut swayidle_timout_args);

            if let Some(resume_action) = resume_action_bat_opt {
                let mut swayidle_resume_args = Self::get_resume_cmd(&resume_action);
                args.append(&mut swayidle_resume_args);
            }
        }

        let pause_audio = format!("playerctl -a pause");
        let before_sleep = format!("{display_off};{lock_screen};{pause_audio};sleep 1");
        let after_resume = display_on.clone();
        let mut before_sleep_args = vec!["before-sleep".to_owned(), before_sleep];
        let mut after_resum_args = vec!["after-resume".to_owned(), after_resume];
        let mut lock_screen_args = vec!["lock".to_owned(), lock_screen];
        args.append(&mut before_sleep_args);
        args.append(&mut after_resum_args);
        args.append(&mut lock_screen_args);
        args
    }

    fn get_timeout_cmd(timeout: u32, action: &str) -> Vec<String> {
        vec![
            "timeout".to_owned(),
            timeout.to_string(),
            action.to_string(),
        ]
    }

    fn get_resume_cmd(action: &str) -> Vec<String> {
        vec!["resume".to_owned(), action.to_owned()]
    }
}

/// A POSIX shell function, `brightness`, that prints the backlight
/// brightness in percent, or sets it to the percentage given as its argument.
///
/// It uses brightnessctl when it is installed, which changes the brightness
/// through logind and needs no setuid binary, and light otherwise.
const BRIGHTNESS_SHELL_FN: &str = r#"brightness() {
    if command -v brightnessctl >/dev/null 2>&1; then
        if [ $# -eq 0 ]; then
            brightnessctl -m -c backlight | cut -d, -f4 | tr -d %
        else
            brightnessctl -q -c backlight set "$1%"
        fi
    elif [ $# -eq 0 ]; then
        light
    else
        light -S "$1"
    fi
}"#;

/// Where the brightness from before the dim is kept, for `undim_cmd`.
const SAVED_BRIGHTNESS: &str = "$XDG_RUNTIME_DIR/screen_brightness_old.var";

/// Shell command that saves the brightness and dims the screen to
/// `idle_brightness` percent, if it is brighter than that.
fn dim_cmd(idle_brightness: i32) -> String {
    format!(
        r#"{BRIGHTNESS_SHELL_FN}
curr_brightness=$(brightness)
echo "$curr_brightness" > "{SAVED_BRIGHTNESS}"
if [ "${{curr_brightness%%.*}}" -gt {idle_brightness} ] 2>/dev/null; then
    brightness {idle_brightness}
fi
"#
    )
}

/// Shell command that restores the brightness saved by `dim_cmd`.
fn undim_cmd() -> String {
    format!(
        r#"{BRIGHTNESS_SHELL_FN}
old_brightness=$(cat "{SAVED_BRIGHTNESS}" 2>/dev/null)
if [ -n "$old_brightness" ]; then
    brightness "$old_brightness"
fi
"#
    )
}

/// Where the power supply class lives in sysfs.
const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";

/// A POSIX shell function, `on_ac`, that succeeds while the machine runs on
/// AC power, read from the power supplies under `dir`.
///
/// It replaces the AC check from Debian's powermgmt-base package, which other
/// distributions such as Fedora don't have. It needs only `sh` and `cat`:
///
/// - a Mains or USB supply that is online means AC;
/// - otherwise a system battery means battery;
/// - a machine with neither (a desktop, a VM) counts as AC, as an unknown
///   power source should.
///
/// Batteries of devices such as wireless mice have `scope` set to `Device`
/// and are ignored.
fn on_ac_shell_fn(dir: &str) -> String {
    format!(
        r#"on_ac() {{
    battery=
    for supply in "{dir}"/*; do
        [ -r "$supply/type" ] || continue
        [ "$(cat "$supply/scope" 2>/dev/null)" = Device ] && continue
        case "$(cat "$supply/type")" in
            Mains | USB*) [ "$(cat "$supply/online" 2>/dev/null)" = 1 ] && return 0 ;;
            Battery) battery=1 ;;
        esac
    done
    [ -z "$battery" ]
}}"#
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerSource {
    Ac,
    Battery,
}

/// Shell command that runs `action` only while on the given power source.
fn power_source_guard(source: PowerSource, action: &str) -> String {
    power_source_guard_in(POWER_SUPPLY_DIR, source, action)
}

fn power_source_guard_in(dir: &str, source: PowerSource, action: &str) -> String {
    let test = match source {
        PowerSource::Ac => "on_ac",
        PowerSource::Battery => "! on_ac",
    };
    format!(
        "{}\nif {test}; then\n    {action}\nfi\n",
        on_ac_shell_fn(dir)
    )
}

#[derive(Debug)]
enum KeySymAction {
    Unbind { key: String },
    ReBind { key: String, action: String },
}

const POWER_OFF_KEY: &str = "XF86Poweroff";

impl PowerSettings {
    // Requires settings HandlePowerKey=ignore in logind.conf
    fn handle_power_btn_action_change(&self) -> Result<(), Box<dyn Error>> {
        use PowerButtonAction::*;
        let mut sway_conn = swayipc::Connection::new()?;
        use KeySymAction::*;
        let btn_change_action = match self.power_button_action() {
            Nothing => Unbind {
                key: POWER_OFF_KEY.to_string(),
            },
            Suspend => ReBind {
                key: POWER_OFF_KEY.to_string(),
                action: "systemctl suspend".to_string(),
            },
            Hibernate => ReBind {
                key: POWER_OFF_KEY.to_string(),
                action: "systemctl hibernate".to_string(),
            },
            Interactive => ReBind {
                key: POWER_OFF_KEY.to_string(),
                // TODO: Replace with a more sensible action (Prefferably user defined)
                action: "swaynag -t warning -m 'Do you really want to shutdown' -b 'Shutdown' '/usr/bin/gnome-session-quit --power-off --no-prompt'".to_string()
            }
        };

        match btn_change_action {
            Unbind { ref key } => sway_conn.run_command(format!("unbindsym {key}"))?,
            ReBind {
                ref key,
                ref action,
            } => {
                let _ = sway_conn.run_command(format!("unbindsym {key}"));
                sway_conn.run_command(format!("bindsym {key} exec \"{action}\""))?
            }
        };

        Ok(())
    }
}
fn main() {
    let app = Application::new(Some("org.regolith.powerd"), ApplicationFlags::IS_SERVICE);

    // Setup holds for event loop and logind inhibit
    let hold = (app.hold(), logind::setup_logind_inhibits());

    // Keep the hold guard alive until the end of the program
    let hold_guard = ManuallyDrop::new(hold);

    let manager = Manager::new();
    manager.run().expect("Failed to run");
    app.run();

    // Drop the hold guard
    let _ = ManuallyDrop::into_inner(hold_guard);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fake /sys/class/power_supply, removed when dropped.
    struct FakeSysfs(PathBuf);

    impl FakeSysfs {
        fn new(name: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "regolith-powerd-test-{}-{}-{name}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            FakeSysfs(dir)
        }

        /// Add a supply with the given attribute files.
        fn supply(self, name: &str, attrs: &[(&str, &str)]) -> Self {
            let dir = self.0.join(name);
            fs::create_dir_all(&dir).unwrap();
            for (attr, value) in attrs {
                fs::write(dir.join(attr), format!("{value}\n")).unwrap();
            }
            self
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for FakeSysfs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Run the AC and battery guards against the tree and report which
    /// action ran.
    fn ran(sysfs: &FakeSysfs) -> (bool, bool) {
        let dir = sysfs.path().to_str().unwrap();
        let run = |source| {
            let cmd = power_source_guard_in(dir, source, "echo ran");
            let out = Command::new("sh").arg("-c").arg(cmd).output().unwrap();
            assert!(out.status.success());
            String::from_utf8(out.stdout).unwrap().trim() == "ran"
        };
        (run(PowerSource::Ac), run(PowerSource::Battery))
    }

    const AC: (bool, bool) = (true, false);
    const BATTERY: (bool, bool) = (false, true);

    /// Run `cmd` with fake brightnessctl (if `with_brightnessctl`) and light
    /// commands that report `current` and log their calls. Returns the calls.
    fn brightness_calls(cmd: &str, with_brightnessctl: bool, current: &str) -> Vec<String> {
        let dir = FakeSysfs::new("brightness");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let log = dir.path().join("calls");
        let percent = current.split('.').next().unwrap();
        let mut tools = vec![(
            "light",
            format!("echo \"light $*\" >> {log:?}; [ $# -eq 0 ] && echo {current}; true"),
        )];
        if with_brightnessctl {
            tools.push((
                "brightnessctl",
                format!(
                    "echo \"brightnessctl $*\" >> {log:?}; \
                     [ \"$1\" = -m ] && echo intel_backlight,backlight,750,{percent}%,1000; true"
                ),
            ));
        }
        for (name, body) in tools {
            let path = bin.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            Command::new("chmod").arg("+x").arg(&path).status().unwrap();
        }
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let out = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .env("PATH", path)
            .env("XDG_RUNTIME_DIR", dir.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn dims_with_brightnessctl_when_installed() {
        let calls = brightness_calls(&dim_cmd(30), true, "75");
        assert_eq!(
            calls,
            [
                "brightnessctl -m -c backlight",
                "brightnessctl -q -c backlight set 30%"
            ]
        );
    }

    #[test]
    fn dims_with_light_otherwise() {
        let calls = brightness_calls(&dim_cmd(30), false, "75.00");
        assert_eq!(calls, ["light ", "light -S 30"]);
    }

    #[test]
    fn leaves_a_darker_screen_alone() {
        assert_eq!(
            brightness_calls(&dim_cmd(30), true, "20"),
            ["brightnessctl -m -c backlight"]
        );
        assert_eq!(brightness_calls(&dim_cmd(30), false, "20.00"), ["light "]);
    }

    #[test]
    fn restores_the_saved_brightness() {
        let cmd = format!("{}\n{}", dim_cmd(30), undim_cmd());
        let calls = brightness_calls(&cmd, true, "75");
        assert_eq!(calls.last().unwrap(), "brightnessctl -q -c backlight set 75%");
        let calls = brightness_calls(&cmd, false, "75.00");
        assert_eq!(calls.last().unwrap(), "light -S 75.00");
    }

    #[test]
    fn no_backlight_is_harmless() {
        let cmd = format!("{}\n{}", dim_cmd(30), undim_cmd());
        // brightnessctl prints nothing without a backlight.
        let calls = brightness_calls(&cmd, true, "");
        assert!(calls.iter().all(|c| !c.contains(" set ")), "{calls:?}");
    }

    #[test]
    fn ac_only() {
        let sysfs = FakeSysfs::new("ac-only").supply("AC", &[("type", "Mains"), ("online", "1")]);
        assert_eq!(ran(&sysfs), AC);
    }

    #[test]
    fn battery_only() {
        let sysfs = FakeSysfs::new("battery-only")
            .supply("BAT0", &[("type", "Battery"), ("status", "Discharging")]);
        assert_eq!(ran(&sysfs), BATTERY);
    }

    #[test]
    fn plugged_in_laptop() {
        let sysfs = FakeSysfs::new("both-online")
            .supply("AC", &[("type", "Mains"), ("online", "1")])
            .supply("BAT0", &[("type", "Battery")]);
        assert_eq!(ran(&sysfs), AC);
    }

    #[test]
    fn unplugged_laptop() {
        let sysfs = FakeSysfs::new("both-offline")
            .supply("AC", &[("type", "Mains"), ("online", "0")])
            .supply("BAT0", &[("type", "Battery")]);
        assert_eq!(ran(&sysfs), BATTERY);
    }

    #[test]
    fn no_supplies_is_ac() {
        let sysfs = FakeSysfs::new("none");
        assert_eq!(ran(&sysfs), AC);
    }

    #[test]
    fn missing_power_supply_class_is_ac() {
        let sysfs = FakeSysfs::new("missing");
        let gone = sysfs.path().join("does-not-exist");
        let cmd = power_source_guard_in(gone.to_str().unwrap(), PowerSource::Ac, "echo ran");
        let out = Command::new("sh").arg("-c").arg(cmd).output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "ran");
    }

    #[test]
    fn usb_c_charger_is_ac() {
        let sysfs = FakeSysfs::new("usb-c")
            .supply("ucsi-source-psy-USBC000:001", &[("type", "USB"), ("online", "1")])
            .supply("BAT0", &[("type", "Battery")]);
        assert_eq!(ran(&sysfs), AC);
    }

    #[test]
    fn several_supplies_one_online() {
        let sysfs = FakeSysfs::new("several")
            .supply("ucsi-source-psy-USBC000:001", &[("type", "USB"), ("online", "0")])
            .supply("ucsi-source-psy-USBC000:002", &[("type", "USB"), ("online", "1")])
            .supply("BAT0", &[("type", "Battery")])
            .supply("BAT1", &[("type", "Battery")]);
        assert_eq!(ran(&sysfs), AC);
    }

    #[test]
    fn device_batteries_are_ignored() {
        let sysfs = FakeSysfs::new("device-battery").supply(
            "hidpp_battery_0",
            &[("type", "Battery"), ("scope", "Device")],
        );
        assert_eq!(ran(&sysfs), AC);
    }
}
