# Installing, updating and restoring Haggle on hp-server

This guide sets up Haggle, the price watcher, on hp-server (Debian or Ubuntu with systemd), with the code in `~/Projects/haggle` and the database on `/mnt/database`.

Run the commands as your own user unless a line starts with `sudo`.

## 1. Toolchain

Install the system packages. `sqlite3` is only needed by the backup unit; the Haggle binary bundles SQLite and uses rustls, so no libssl is needed.

```
sudo apt install build-essential pkg-config git curl sqlite3
```

Install Rust with rustup, choosing the default `stable` toolchain when asked:

```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
```

The first release build on the i5-8500T is slow. Let it finish; later builds reuse the compiled dependencies and are much faster.

## 2. One-time setup

Create the system user that runs Haggle. It has no login shell and no home directory.

```
sudo useradd --system --no-create-home --shell /usr/sbin/nologin haggle
```

Create the database and backup directories on the database volume and give them to the `haggle` user:

```
sudo mkdir -p /mnt/database/haggle/backups
sudo chown -R haggle:haggle /mnt/database/haggle
```

Create the settings file that the service reads. It is owned by root and readable only by root. Systemd reads it as root when it starts the service.

```
sudo mkdir -p /etc/haggle
sudo install -m 600 -o root -g root /dev/null /etc/haggle/haggle.env
sudoedit /etc/haggle/haggle.env
```

Put these two lines in the file:

```
HAGGLE_DB=/mnt/database/haggle/haggle.db
HAGGLE_LISTEN=192.168.5.21:8421
```

The email settings are shared with isp-speed-monitor. The service also reads `/opt/speedtest/.env`, which must define `GMAIL_USER`, `GMAIL_APP_PASSWORD` and `ALERT_EMAIL`: all three or none. With none of them set, notifications are off and the journal shows a warning. The app password exists only in that file. Do not copy it into the repository, the README or shell history.

To see that notifications work, check that `journalctl -u haggle` shows no "notifications are off" warning. A price drop then arrives in the `ALERT_EMAIL` inbox, sent from `GMAIL_USER`.

## 3. Install

Clone the code and build it. `make check` runs formatting, clippy, the release build and the tests; `make install` copies the binary to `/usr/local/bin` and the units in `deploy/` to `/etc/systemd/system`, then reloads systemd.

```
mkdir -p ~/Projects
git clone https://github.com/NC4321/haggle.git ~/Projects/haggle
cd ~/Projects/haggle
make check && make install
```

Start the service and the daily backup timer, and make both start at boot:

```
sudo systemctl enable --now haggle.service haggle-backup.timer
```

Check that it runs:

```
systemctl status haggle
journalctl -u haggle -f
```

Open the web UI at http://192.168.5.21:8421 from a machine on the LAN.

## 4. Adding watches from the shell

The `haggle` command writes to the same database as the service, so it must run as the `haggle` user, with `HAGGLE_DB` set. Run it from `/`, because the `haggle` user cannot enter your home directory:

```
cd / && sudo -u haggle env HAGGLE_DB=/mnt/database/haggle/haggle.db /usr/local/bin/haggle add https://SHOP/PRODUCT --name "Kettle" --target 199.99
```

The other commands take the same prefix, for example `haggle ls` or `haggle history ID`. The service picks up new watches without a restart.

## 5. Update

Pull the new code, check it, install it, and restart the service so it runs the new binary:

```
cd ~/Projects/haggle
git pull && make check && make install
sudo systemctl restart haggle
```

`make install` reloads systemd, so changed units take effect; the service needs the restart to run the new binary.

When upgrading an existing install, remove the old notification lines (the two `HAGGLE_` variables other than `HAGGLE_DB` and `HAGGLE_LISTEN`) from `/etc/haggle/haggle.env`, then run `sudo systemctl daemon-reload && sudo systemctl restart haggle`.

## 6. Backups

The `haggle-backup.timer` starts `haggle-backup.service` once a day. The service writes a consistent copy of the database with `VACUUM INTO` to `/mnt/database/haggle/backups/haggle-YYYYMMDD-HHMMSS.db`, even while the service is writing. It then deletes all but the newest seven copies. An old copy is deleted only after the new one has been written.

Check when the next backup runs:

```
systemctl list-timers haggle-backup.timer
```

Run a backup now, outside the schedule:

```
sudo systemctl start haggle-backup.service
ls -1t /mnt/database/haggle/backups
```

## 7. Restore

Restoring replaces the current database. Changes made after the chosen backup are lost. Pick the backup to restore from the list in `/mnt/database/haggle/backups`, then run these commands, replacing the file name with your choice:

```
sudo systemctl stop haggle
sudo cp /mnt/database/haggle/backups/haggle-20261115-030012.db /mnt/database/haggle/haggle.db
sudo rm -f /mnt/database/haggle/haggle.db-wal /mnt/database/haggle/haggle.db-shm
sudo chown haggle:haggle /mnt/database/haggle/haggle.db
sudo systemctl start haggle
cd / && sudo -u haggle env HAGGLE_DB=/mnt/database/haggle/haggle.db /usr/local/bin/haggle ls
```

The old WAL and shared-memory files must go: they belong to the old database, and SQLite would apply them to the restored copy. `haggle ls` should list the watches as they were at the time of the backup.

## 8. Troubleshooting

- Logs: `journalctl -u haggle -n 200` for the service, and `journalctl -u haggle-backup.service` for the backups.
- `haggle ls` shows each watch's status. `needs-selector` means the page has no price the extractor can find: test a CSS selector with `cd / && sudo -u haggle env HAGGLE_DB=/mnt/database/haggle/haggle.db /usr/local/bin/haggle check URL --selector CSS`, then remove the watch with `haggle rm ID` and add it again with `--selector`, both with the prefix from section 4. `blocked` means the shop refused the check, through robots.txt or with a 403.
- Exit code 2 means a configuration error, such as a missing `HAGGLE_DB` or a bad `/etc/haggle/haggle.env`. Exit code 1 is any other error.
