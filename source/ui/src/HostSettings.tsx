import { useEffect, useState } from "react";
type Info = {
  addresses: string[];
  networks: { name: string; ip: string }[];
  interface: string | null;
  version: string;
};
export function HostSettings() {
  const [info, setInfo] = useState<Info | null>(null);
  const [selected, setSelected] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
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
          <option value="">Manual IP connections</option>
          {info?.networks.map((n) => (
            <option key={`${n.name}@${n.ip}`} value={n.name}>
              {n.name} · {n.ip}
            </option>
          ))}
        </select>
      </label>
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
      {message && <p role="status">{message}</p>}
    </section>
  );
}
