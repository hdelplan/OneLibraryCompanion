import { useState } from "react";
import { SubnetSearch } from "./SubnetSearch";
export type DirectPeer = {
  number: number;
  ip: string;
  state: "connecting" | "connected" | "error";
  error: string | null;
  usb: boolean;
};
function savedIp(number: number) {
  try {
    return (
      localStorage.getItem(`pc.cdj-ip.${number}`) ??
      (number === 1 ? localStorage.getItem("pc.manual-ip") : "") ??
      ""
    );
  } catch {
    return "";
  }
}
export function ManualLibrary({
  onConnected,
  peers,
  liveActive,
}: {
  onConnected: (source?: string) => void;
  peers: DirectPeer[];
  liveActive: boolean;
}) {
  const [ips, setIps] = useState(() => [savedIp(1), savedIp(2)]);
  const [busy, setBusy] = useState<number | null>(null);
  const [error, setError] = useState<{
    number: number;
    message: string;
  } | null>(null);
  async function act(
    number: number,
    action: "connect" | "disconnect" | "library",
  ) {
    const ip = ips[number - 1].trim();
    setBusy(number);
    setError(null);
    try {
      const response = await fetch(
        action === "library" ? "/api/library/manual" : "/api/live/direct",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            number,
            ip,
            disconnect: action === "disconnect",
          }),
        },
      );
      if (!response.ok) throw new Error(await response.text());
      if (action !== "disconnect") {
        try {
          localStorage.setItem(`pc.cdj-ip.${number}`, ip);
        } catch {
          /* Connection still works. */
        }
      }
      onConnected(action === "library" ? "direct" : undefined);
    } catch (e) {
      setError({ number, message: String(e) });
    } finally {
      setBusy(null);
    }
  }
  return (
    <div className="direct-connections" aria-label="CDJ connections">
      <SubnetSearch
        disabled={[1, 2].map(
          (number) =>
            busy !== null ||
            peers.some((p) => p.number === number && p.state !== "error"),
        )}
        onSelect={(number, ip) =>
          setIps((current) =>
            current.map((value, index) => (index === number - 1 ? ip : value)),
          )
        }
      />
      <div className="direct-connection-rows">
        {[1, 2].map((number) => {
          const peer = peers.find((p) => p.number === number);
          const connected = !!peer && peer.state !== "error";
          return (
            <div className="direct-connection" key={number}>
              <label htmlFor={`cdj-ip-${number}`}>CDJ {number}</label>
              <input
                id={`cdj-ip-${number}`}
                aria-label={`CDJ ${number} IP address`}
                inputMode="decimal"
                autoComplete="off"
                spellCheck={false}
                placeholder={number === 1 ? "10.80.1.200" : "CDJ 2 IP address"}
                value={connected ? peer.ip : ips[number - 1]}
                disabled={connected || busy !== null}
                onChange={(e) =>
                  setIps((current) =>
                    current.map((value, i) =>
                      i === number - 1 ? e.target.value : value,
                    ),
                  )
                }
              />
              <button
                disabled={
                  busy !== null || (!connected && !ips[number - 1].trim())
                }
                onClick={() =>
                  void act(number, connected ? "disconnect" : "connect")
                }
              >
                {busy === number
                  ? "Please wait…"
                  : connected
                    ? "Disconnect"
                    : peer
                      ? "Reconnect"
                      : "Connect"}
              </button>
              {peer?.state === "error" && (
                <button
                  disabled={busy !== null}
                  onClick={() => void act(number, "disconnect")}
                >
                  Disconnect
                </button>
              )}
              {!liveActive && (
                <button
                  disabled={busy !== null || !ips[number - 1].trim()}
                  onClick={() => void act(number, "library")}
                >
                  USB preview
                </button>
              )}
              <span
                className={`direct-state ${peer?.state ?? ""}`}
                role="status"
              >
                {peer?.state === "connected"
                  ? `Connected · ${peer.usb ? "USB present" : "No local USB"}`
                  : peer?.state === "connecting"
                    ? "Waiting for status…"
                    : peer?.error || "Not connected"}
              </span>
              {error?.number === number && (
                <span role="alert" className="library-error">
                  {error.message}
                </span>
              )}
            </div>
          );
        })}
      </div>
      <small>
        Match CDJ 1 / CDJ 2 to the players’ physical numbers. Connect both to
        the same Pro DJ Link network. Either connected USB can load either
        stopped player.
      </small>
    </div>
  );
}
