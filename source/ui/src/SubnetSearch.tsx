import { useCallback, useEffect, useRef, useState } from "react";
type Network = {
  id: string;
  name: string;
  ip: string;
  cidr: string;
  hosts: number;
};
type Result = { ip: string; service: string };
export function SubnetSearch({
  onSelect,
  disabled,
}: {
  onSelect: (number: number, ip: string) => void;
  disabled: boolean[];
}) {
  const [networks, setNetworks] = useState<Network[]>([]);
  const [selected, setSelected] = useState("");
  const [results, setResults] = useState<Result[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const controller = useRef<AbortController | null>(null);
  useEffect(() => () => controller.current?.abort(), []);
  const refresh = useCallback(async () => {
    try {
      const response = await fetch("/api/network/search");
      if (!response.ok) throw new Error(await response.text());
      const values: Network[] = await response.json();
      setNetworks(values);
      setSelected((old) =>
        values.some((n) => n.id === old) ? old : (values[0]?.id ?? ""),
      );
      setMessage(
        values.length
          ? "Choose the network connected to your CDJs."
          : "No local IPv4 subnet found. Connect Wi-Fi or Ethernet, then refresh. Supported networks: /16 to /30.",
      );
    } catch (error) {
      setMessage(String(error));
    }
  }, []);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  async function search() {
    const abort = new AbortController();
    controller.current = abort;
    setBusy(true);
    setResults([]);
    setMessage("Searching…");
    let found = 0;
    try {
      const response = await fetch("/api/network/search", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ network: selected }),
        signal: abort.signal,
      });
      if (!response.ok) throw new Error(await response.text());
      if (!response.body) throw new Error("Search response unavailable.");
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let pending = "";
      let completed = 0,
        total = 0;
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        pending += decoder.decode(value, { stream: true });
        let newline: number;
        while ((newline = pending.indexOf("\n")) >= 0) {
          const event = JSON.parse(pending.slice(0, newline)) as {
            completed: number;
            total: number;
            ip: string;
            service: string | null;
          };
          pending = pending.slice(newline + 1);
          completed = event.completed;
          total = event.total;
          if (event.service) {
            found++;
            setResults((old) => [
              ...old,
              { ip: event.ip, service: event.service! },
            ]);
          }
          setMessage(
            `Searching ${completed} / ${total} addresses · ${found} found`,
          );
        }
      }
      if (!total || completed !== total)
        throw new Error(
          "Search interrupted. Refresh the network list and try again.",
        );
      setMessage(
        found
          ? `Search complete · ${found} found. Select an address, then Connect to verify the player number.`
          : "No CDJs found. Check Local Network permission, the selected network, and bridge settings. You can still enter addresses manually.",
      );
    } catch (error) {
      setMessage(abort.signal.aborted ? "Search stopped." : String(error));
    } finally {
      controller.current = null;
      setBusy(false);
    }
  }
  return (
    <div className="subnet-search">
      <div className="subnet-search-controls">
        <button disabled={busy} onClick={() => void refresh()}>
          Refresh networks
        </button>
        <select
          aria-label="CDJ search network"
          value={selected}
          disabled={busy}
          onChange={(e) => setSelected(e.target.value)}
        >
          <option value="" disabled>
            Select network
          </option>
          {networks.map((n) => (
            <option key={n.id} value={n.id}>
              {n.name} · {n.cidr} · {n.hosts} addresses
            </option>
          ))}
        </select>
        <button disabled={!selected || busy} onClick={() => void search()}>
          Search for CDJs
        </button>
        {busy && (
          <button onClick={() => controller.current?.abort()}>
            Stop search
          </button>
        )}
      </div>
      <small>
        Search every host address on the selected network using direct queries.
        Large subnets can take several minutes.
      </small>
      <p role="status">{message}</p>
      {results.map((result) => (
        <div className="subnet-search-result" key={result.ip}>
          <span>
            <strong>{result.ip}</strong> · {result.service}
          </span>
          {[1, 2].map((number) => (
            <button
              key={number}
              disabled={disabled[number - 1]}
              onClick={() => onSelect(number, result.ip)}
            >
              Use for CDJ {number}
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
