import { useEffect, useRef, useState } from "react";
type Info = {
  addresses: string[];
  networks: { name: string; ip: string }[];
  interface: string | null;
  version: string;
  hostControls?: boolean;
};
export function HostSettings() {
  const [info, setInfo] = useState<Info | null>(null);
  const [selected, setSelected] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [action, setAction] = useState<"restart" | "shutdown" | "usb" | null>(
    null,
  );
  const [usbVolumes, setUsbVolumes] = useState<
    { device: string; label: string }[]
  >([]);
  const [usbDevice, setUsbDevice] = useState("");
  const [usbJob, setUsbJob] = useState(
    () => sessionStorage.getItem("olc-usb-job") ?? "",
  );
  useEffect(() => {
    if (!info?.hostControls) return;
    let cancelled = false;
    const poll = async () => {
      try {
        const response = await fetch("/api/app/usb");
        if (!response.ok) return;
        const next = await response.json();
        if (cancelled) return;
        setUsbVolumes(next.volumes);
        if (usbJob && next.result?.job === usbJob) {
          setMessage(next.result.message);
          if (next.result.state !== "running") {
            setUsbJob("");
            sessionStorage.removeItem("olc-usb-job");
            setBusy(false);
          }
        }
      } catch {
        /* OLC briefly stops while the USB is unmounted. */
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 1500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [info?.hostControls, usbJob]);
  const confirmation = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    if (action && confirmation.current && !confirmation.current.open)
      confirmation.current.showModal();
  }, [action]);
  async function confirmAction() {
    if (!action || busy) return;
    const confirmedAction = action;
    setAction(null);
    setBusy(true);
    try {
      const response = await fetch(
        confirmedAction === "usb" ? "/api/app/usb" : "/api/app/control",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(
            confirmedAction === "usb"
              ? { device: usbDevice, confirmed: true }
              : { action: confirmedAction, confirmed: true },
          ),
        },
      );
      if (!response.ok) throw Error(await response.text());
      if (confirmedAction === "usb") {
        const next = await response.json();
        sessionStorage.setItem("olc-usb-job", next.job);
        setUsbJob(next.job);
        setMessage(
          "Unmounting USB. Keep it connected until OLC confirms it is safe to remove.",
        );
        return;
      }
      setMessage(
        confirmedAction === "restart"
          ? "OLC is restarting. Refresh this page in a few seconds."
          : "The host is shutting down. Wait for shutdown to finish before unplugging power.",
      );
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  }
  const [discovery, setDiscovery] = useState<{
    state: string;
    interface?: string;
    ip?: string;
    error?: string;
  } | null>(null);
  useEffect(() => {
    let cancelled = false;
    const refresh = async () => {
      try {
        const response = await fetch("/api/live");
        if (!response.ok) return;
        const live = await response.json();
        if (!cancelled) setDiscovery({ ...live.discovery, error: live.error });
      } catch {
        /* Host connectivity is reported by the main live view. */
      }
    };
    void refresh();
    const timer = window.setInterval(refresh, 3000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);
  useEffect(() => {
    let cancelled = false;
    fetch("/api/app")
      .then((r) => {
        if (!r.ok) throw Error("Host settings unavailable");
        return r.json();
      })
      .then((next: Info) => {
        if (!cancelled) {
          setInfo(next);
          setSelected(next.interface ?? "");
        }
      })
      .catch((e) => {
        if (!cancelled) setMessage(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);
  return (
    <section className="settings-panel">
      <h2>OneLibraryCompanion {info?.version}</h2>
      <p>
        Web access is available to every device on this network, without
        pairing.
      </p>
      {info?.addresses.map((address) => (
        <p key={address}>
          <a href={address} target="_blank" rel="noreferrer">
            {address}
          </a>
        </p>
      ))}
      {info && info.addresses.length === 0 && (
        <p>
          No LAN address available. Check the network connection and host listen
          address.
        </p>
      )}
      <label>
        CDJ discovery interface
        <select value={selected} onChange={(e) => setSelected(e.target.value)}>
          <option value="auto">Automatic discovery</option>
          <option value="">Manual IP connections</option>
          {info?.networks.map((n) => (
            <option key={`${n.name}@${n.ip}`} value={n.name}>
              {n.name} · {n.ip}
            </option>
          ))}
        </select>
      </label>
      {discovery?.state === "searching" && (
        <p role="status">{discovery.error || "Searching for CDJs…"}</p>
      )}
      {discovery?.state === "connected" && (
        <p role="status">
          CDJ network: {discovery.interface} · {discovery.ip}
        </p>
      )}
      <p>
        Automatic discovery listens for CDJs on your local network. Connect to
        the same switch or network as the decks.
      </p>
      <button
        disabled={!info || busy}
        onClick={async () => {
          setBusy(true);
          setMessage("");
          try {
            const r = await fetch("/api/app", {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ interface: selected || null }),
            });
            if (!r.ok) throw Error(await r.text());
            setMessage(
              "Saved. Quit and reopen OLC to apply. Environment overrides take precedence.",
            );
          } catch (e) {
            setMessage(String(e));
          } finally {
            setBusy(false);
          }
        }}
      >
        Save connection mode
      </button>
      <p>
        Closing the desktop window keeps web access running. Use Quit OLC to
        stop the service.
      </p>
      {info?.hostControls && (
        <div>
          {usbVolumes.map((volume) => (
            <button
              key={volume.device}
              disabled={busy || !!usbJob}
              onClick={() => {
                setUsbDevice(volume.device);
                setAction("usb");
              }}
            >
              Unmount USB · {volume.label}
            </button>
          ))}
          {(["restart", "shutdown"] as const).map((action) => (
            <button
              key={action}
              disabled={busy || !!usbJob}
              onClick={() => setAction(action)}
            >
              {action === "restart" ? "Restart OLC" : "Shut down host"}
            </button>
          ))}
        </div>
      )}
      {action && (
        <dialog
          ref={confirmation}
          className="load-confirmation"
          aria-labelledby="host-action-title"
          aria-describedby="host-action-description"
          onCancel={() => setAction(null)}
        >
          <h2 id="host-action-title">
            {action === "usb"
              ? "Unmount USB?"
              : action === "restart"
                ? "Restart OLC?"
                : "Shut down host?"}
          </h2>
          <p id="host-action-description">
            {action === "usb"
              ? "Turn off or disconnect the CDJs first. OLC will stop briefly to unmount this USB. Keep it plugged in until OLC confirms you can remove it. Other apps using the USB may prevent unmounting."
              : action === "restart"
                ? "Stop playback on players using OLC USB tracks before restarting. They may enter an emergency loop and require the track to be reloaded. All connected screens will disconnect briefly."
                : "Stop playback on players using OLC USB tracks before shutting down. They may enter an emergency loop and require the track to be reloaded. This powers off the host; power must be restored manually."}
          </p>
          <div>
            <button autoFocus onClick={() => setAction(null)}>
              Cancel
            </button>
            <button
              className="load-danger"
              disabled={busy}
              onClick={() => void confirmAction()}
            >
              {action === "usb"
                ? "Unmount USB"
                : action === "restart"
                  ? "Restart OLC"
                  : "Shut down host"}
            </button>
          </div>
        </dialog>
      )}
      {message && <p role="status">{message}</p>}
    </section>
  );
}
