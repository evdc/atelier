# Atelier

An agentic development environment: a server (`atelierd`) that runs coding agents in git worktrees and logs
everything they do, plus thin clients (desktop, iPhone) that attach to it. `DESIGN.md` explains why it's built this
way; `CLAUDE.md` is the guide for working on it. `make help` lists the dev tasks.

## Run on your iPhone

The phone is a pure client: it connects to the `atelierd` on your Mac over Tailscale and never runs agents itself.
You need a Mac with Xcode, an iPhone, Tailscale on both (signed in to the **same** Tailscale account), and an Apple
ID added to Xcode (Xcode → Settings → Accounts; a free personal team works, but its builds expire after 7 days).

### 1. Let the server listen on your tailnet

The server only listens on `127.0.0.1` unless told otherwise. Restart it with `ATELIER_LISTEN_TAILSCALE=1` so it
also listens on the Mac's tailnet address (never on `0.0.0.0`). This uses your real data in `~/.atelier`.

```sh
cargo build --release -p atelier-server
pkill -x atelierd                # stops running agents; their sessions can be continued afterwards
ATELIER_LISTEN_TAILSCALE=1 nohup ./target/release/atelierd >> ~/.atelier/atelierd.log 2>&1 &
tail -n 3 ~/.atelier/atelierd.log   # expect: listening on 100.x.y.z:7747 (tailnet)
```

The desktop app connects to whatever is already listening on `127.0.0.1:7747`, so it keeps working as before. If
the desktop app has to start the server itself it won't set the variable, so repeat this step after a reboot (until
the server runs as a login agent, AT-011).

**Who gets in:** loopback connections need the token in `~/.atelier/token`. On the tailnet address, a device signed
in to the same Tailscale user as the Mac is let in without a token (`tailscale whois`); tagged or shared devices
need the token.

### 2. Find your signing team ID

```sh
security find-certificate -c "Apple Development" -p | openssl x509 -noout -subject
```

The team ID is the `OU=` value (10 characters). If you have no Apple Development certificate yet, open
`app/gen/apple/atelier-app.xcodeproj` once, select the `atelier-app_iOS` target → Signing & Capabilities, pick your
team, and let Xcode create one.

### 3. Prepare the phone (first time only)

1. Connect it by USB and tap **Trust** on the phone.
2. Turn on **Settings → Privacy & Security → Developer Mode** (the phone restarts).
3. Check the Mac sees it: `xcrun devicectl list devices` should show it as `available (paired)`. After this, the
   phone can stay on Wi-Fi.
4. Register the phone with your team and create the provisioning profile (otherwise signing fails with "Your team
   has no devices from which to generate a provisioning profile"). Xcode does this when you first run to the phone
   from the IDE; from the command line:

   ```sh
   cd app/gen/apple && xcodebuild -project atelier-app.xcodeproj -scheme atelier-app_iOS -configuration release \
     -destination 'platform=iOS,name=<phone name>' -allowProvisioningUpdates -allowProvisioningDeviceRegistration \
     DEVELOPMENT_TEAM=<team id> build; cd -
   ```

   It may fail later in the Rust build step when run outside the Tauri CLI; that's fine, the profile is created
   before that.

### 4. Build, install and launch

```sh
export APPLE_DEVELOPMENT_TEAM=<team id>
make ios-device IOS_DEVICE="<phone name>"    # the name from devicectl
```

This builds the web UI and a signed release IPA (`app/gen/apple/build/arm64/Atelier.ipa`), installs it with
`devicectl` and launches it. The first launch fails with "invalid code signature … not been explicitly trusted by
the user": on the phone, go to **Settings → General → VPN & Device Management**, trust your Apple ID, then tap the
Atelier icon. (`cargo tauri ios run` doesn't work for devices with tauri-cli 2.12, as its export step fails, which
is why the target doesn't use it.) If signing fails because `sh.atelier.app` is taken (it has to be unique across
Apple), change `identifier` in `app/tauri.ios.conf.json`. The build writes your team ID into
`app/gen/apple/atelier-app.xcodeproj/project.pbxproj`; don't commit that change.

### 5. Connect

On first launch the app shows **Settings**. Enter the Mac's tailnet address (`tailscale ip -4` on the Mac, or its
MagicDNS name, such as `my-mac`), leave the token empty, and tap **Connect**. Port 7747 is assumed unless you give
one. The app remembers the server; to change it later, tap the connection dot in the footer.

### Troubleshooting

- **Can't reach the server:** is Tailscale connected on the phone? Does `tail ~/.atelier/atelierd.log` show the
  tailnet listener? Is a firewall on the Mac blocking incoming connections to `atelierd`?
- **"This device isn't signed in to the server's Tailscale account":** the phone is on a different Tailscale user
  or is a tagged device. Paste the token from `~/.atelier/token` into Settings.
- **Simulator instead of a phone:** `make ios-run` builds and launches it in the simulator (`IOS_SIM=...` to pick
  one). The simulator shares the Mac's network, so `127.0.0.1` works there, with the token.
