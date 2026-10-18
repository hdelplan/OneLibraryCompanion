import { useEffect, useState } from "react";
import "./cdjConfigurations.css";

type Profile = {
  id: string;
  name: string;
  notes: string;
  revision: number;
  origin: string;
  values: Record<string, number>;
};
type Player = {
  number: number;
  name: string;
  ip: string;
  connection: string;
  playing: boolean;
  statusAgeMs: number | null;
  configurationIdentity: string;
  configurationUsb: boolean;
  configurationSd: boolean;
};
type Automatic = {
  enabled: boolean;
  defaultProfile: string | null;
  players: Record<string, string>;
};
type State = {
  revision: number;
  profiles: Profile[];
  fields: {
    id: string;
    label: string;
    group: string;
    options: { value: number; label: string }[];
  }[];
  players: Player[];
  automatic: Automatic;
  error: string | null;
  events: {
    at: number;
    detail: {
      outcome?: string;
      message?: string;
      target?: number;
      profile?: string;
      automatic?: boolean;
    };
  }[];
};
async function api(url: string, body?: unknown): Promise<any> {
  const r = await fetch(
    url,
    body === undefined
      ? undefined
      : {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
  );
  if (!r.ok) throw Error(await r.text());
  return r.json();
}
const endpoint = "/api/cdj-configurations";
export function CdjConfigurations() {
  const [state, setState] = useState<State | null>(null);
  const [selected, setSelected] = useState("");
  const [draft, setDraft] = useState<Profile | null>(null);
  const [dirty, setDirty] = useState(false);
  const [automatic, setAutomatic] = useState<Automatic | null>(null);
  const [autoDirty, setAutoDirty] = useState(false);
  const [autoRevision, setAutoRevision] = useState(0);
  const [playerId, setPlayerId] = useState("");
  const [slot, setSlot] = useState("usb");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const refresh = async () => {
    const next = (await api(endpoint)) as State;
    setState(next);
    return next;
  };
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = (await api(endpoint)) as State;
        if (!stopped) setState(next);
      } catch (e) {
        if (!stopped) setError(String(e));
      }
      if (!stopped) timer = setTimeout(() => void poll(), 2000);
    };
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, []);
  useEffect(() => {
    if (!state) return;
    if (!dirty) {
      const p =
        state.profiles.find((p) => p.id === selected) ??
        state.profiles[0] ??
        null;
      setSelected(p?.id ?? "");
      setDraft(p ? { ...p, values: { ...p.values } } : null);
    }
    if (!autoDirty) {
      setAutomatic({
        ...state.automatic,
        players: { ...state.automatic.players },
      });
      setAutoRevision(state.revision);
    }
    if (!state.players.some((p) => p.configurationIdentity === playerId))
      setPlayerId(state.players[0]?.configurationIdentity ?? "");
  }, [state, selected, dirty, autoDirty, playerId]);
  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await work();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const mutate = async (body: unknown) => {
    const result = await api(endpoint, body);
    await refresh();
    return result;
  };
  const select = (id: string) => {
    setDirty(false);
    setSelected(id);
  };
  const edit = (patch: Partial<Profile>) => {
    if (draft) {
      setDraft({ ...draft, ...patch });
      setDirty(true);
    }
  };
  const setAuto = (patch: Partial<Automatic>) => {
    if (automatic) {
      setAutomatic({ ...automatic, ...patch });
      setAutoDirty(true);
    }
  };
  const player = state?.players.find(
    (p) => p.configurationIdentity === playerId,
  );
  const groups = [...new Set(state?.fields.map((f) => f.group) ?? [])];
  const selectImported = async (result: {
    id: string;
    checksumMatches?: boolean;
  }) => {
    setDirty(false);
    setSelected(result.id);
    await refresh();
    setNotice(
      result.checksumMatches === false
        ? "Imported saved settings. The file checksum does not match; its original bytes have been preserved."
        : "Saved settings imported. Review the profile before pushing it.",
    );
  };
  const download = async () => {
    if (!draft) return;
    const result = await api(
      `${endpoint}/${encodeURIComponent(draft.id)}/export`,
    );
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(result, null, 2)], { type: "application/json" }),
    );
    const a = document.createElement("a");
    a.href = url;
    a.download = `${draft.name.replace(/[^a-z0-9_-]+/gi, "-")}.olc-cdj.json`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  return (
    <section className="settings-panel cdj-configurations">
      <h2>CDJ configurations</h2>
      <p>
        Save My Settings profiles on this OLC host, edit them, and push them to
        connected CDJ-2000NXS players.
      </p>
      <div className="cdj-configuration-management">
        {error && (
          <p className="cdj-config-error" role="alert">
            {error}
          </p>
        )}
        {state?.error && (
          <p className="cdj-config-error" role="alert">
            {state.error}
          </p>
        )}
        {notice && (
          <p className="cdj-config-notice" role="status">
            {notice}
          </p>
        )}
        {!state ? (
          <p>Loading configurations…</p>
        ) : (
          <>
            <div className="cdj-config-toolbar">
              <button
                disabled={busy || dirty || !!state.error}
                onClick={() =>
                  void run(async () => {
                    const r = await mutate({
                      action: "create",
                      name: "New CDJ profile",
                    });
                    select(r.id);
                  })
                }
              >
                New profile
              </button>
              <label className="cdj-config-upload">
                Import file
                <input
                  aria-label="Import CDJ settings file"
                  type="file"
                  accept=".dat,.json"
                  disabled={busy || dirty || !!state.error}
                  onChange={(e) => {
                    const input = e.currentTarget;
                    const file = input.files?.[0];
                    if (!file) return;
                    void run(async () => {
                      let result;
                      if (file.name.toLowerCase().endsWith(".json")) {
                        if (file.size > 32768)
                          throw Error("Profile file exceeds 32 KiB");
                        result = await api(
                          `${endpoint}/import-profile`,
                          JSON.parse(await file.text()),
                        );
                      } else {
                        if (file.size > 4096)
                          throw Error("Import MYSETTING.DAT, up to 4 KiB");
                        const r = await fetch(
                          `${endpoint}/import-file?name=${encodeURIComponent("Imported CDJ settings")}`,
                          { method: "POST", body: await file.arrayBuffer() },
                        );
                        if (!r.ok) throw Error(await r.text());
                        result = await r.json();
                      }
                      await selectImported(result);
                      input.value = "";
                    });
                  }}
                />
              </label>
            </div>
            <div className="cdj-config-workspace">
              <aside>
                <h3>Saved profiles</h3>
                {state.profiles.length === 0 && (
                  <p>Create a profile or import saved My Settings.</p>
                )}
                {state.profiles.map((p) => (
                  <button
                    key={p.id}
                    className={p.id === selected ? "selected" : ""}
                    aria-pressed={p.id === selected}
                    disabled={busy || (dirty && p.id !== selected)}
                    onClick={() => select(p.id)}
                  >
                    {p.name}
                  </button>
                ))}
                <p>
                  Profiles belong to the OLC host. Export a profile to transfer
                  it to another host.
                </p>
              </aside>
              <div className="cdj-config-editor">
                {draft ? (
                  <>
                    <label>
                      Profile name
                      <input
                        value={draft.name}
                        maxLength={80}
                        disabled={busy}
                        onChange={(e) => edit({ name: e.target.value })}
                      />
                    </label>
                    <label>
                      Notes
                      <textarea
                        value={draft.notes}
                        maxLength={2000}
                        rows={2}
                        disabled={busy}
                        onChange={(e) => edit({ notes: e.target.value })}
                      />
                    </label>
                    <p className="cdj-config-origin">{draft.origin}</p>
                    {groups.map((group) => (
                      <fieldset key={group}>
                        <legend>{group}</legend>
                        <div className="cdj-config-fields">
                          {state.fields
                            .filter((f) => f.group === group)
                            .map((f) => {
                              const value = draft.values[f.id];
                              const known = f.options.some(
                                (o) => o.value === value,
                              );
                              return (
                                <label key={f.id}>
                                  {f.label}
                                  <select
                                    value={value}
                                    disabled={busy}
                                    onChange={(e) =>
                                      edit({
                                        values: {
                                          ...draft.values,
                                          [f.id]: Number(e.target.value),
                                        },
                                      })
                                    }
                                  >
                                    {!known && (
                                      <option value={value}>
                                        Unknown saved value (0x
                                        {value.toString(16)}) — preserved
                                      </option>
                                    )}
                                    {f.options.map((o) => (
                                      <option key={o.value} value={o.value}>
                                        {o.label}
                                      </option>
                                    ))}
                                  </select>
                                </label>
                              );
                            })}
                        </div>
                      </fieldset>
                    ))}
                    <p>
                      Push sends the complete saved My Settings profile.
                      Unedited and unknown fields are preserved. Application has
                      no known network acknowledgement.
                    </p>
                    <div className="cdj-config-toolbar">
                      <button
                        disabled={!dirty || busy || !!state.error}
                        onClick={() =>
                          void run(async () => {
                            await mutate({ action: "update", ...draft });
                            setDirty(false);
                            setNotice("Profile saved on the OLC host.");
                          })
                        }
                      >
                        Save profile
                      </button>
                      <button
                        disabled={!dirty || busy}
                        onClick={() => setDirty(false)}
                      >
                        Discard edits
                      </button>
                      <button
                        disabled={dirty || busy}
                        onClick={() =>
                          void run(async () => {
                            const r = await mutate({
                              action: "duplicate",
                              id: draft.id,
                              revision: draft.revision,
                              name: `${draft.name.slice(0, 70)} copy`,
                            });
                            select(r.id);
                          })
                        }
                      >
                        Duplicate
                      </button>
                      <button
                        disabled={dirty || busy}
                        onClick={() => void run(download)}
                      >
                        Export profile
                      </button>
                      <button
                        disabled={dirty || busy || !!state.error}
                        onClick={() => {
                          if (
                            !window.confirm(
                              `Delete “${draft.name}”? Its automatic assignments will also be removed.`,
                            )
                          )
                            return;
                          void run(async () => {
                            await mutate({
                              action: "delete",
                              id: draft.id,
                              revision: draft.revision,
                            });
                            select("");
                            setNotice("Profile deleted.");
                          });
                        }}
                      >
                        Delete profile
                      </button>
                    </div>
                    {dirty && (
                      <p role="status">
                        Unsaved edits. Save or discard them before choosing
                        another profile or pushing.
                      </p>
                    )}
                  </>
                ) : (
                  <p>Select or create a profile to edit its options.</p>
                )}
              </div>
            </div>
            <div className="cdj-config-connections">
              <h3>Connected players</h3>
              {state.players.length === 0 && (
                <p>Connect a player in OLC to import or push settings.</p>
              )}
              {state.players.map((p) => (
                <div
                  className="cdj-config-player"
                  key={p.configurationIdentity || p.number}
                >
                  <div>
                    <strong>CDJ {p.number}</strong>
                    <span>
                      {p.name} · {p.ip}
                    </span>
                    <span>
                      {p.connection !== "connected"
                        ? "Waiting for fresh connection"
                        : p.playing
                          ? "Playing — push unavailable"
                          : "Connected"}
                    </span>
                  </div>
                  <button
                    disabled={
                      busy ||
                      dirty ||
                      !draft ||
                      p.name !== "CDJ-2000nexus" ||
                      p.connection !== "connected" ||
                      p.playing ||
                      p.statusAgeMs === null ||
                      p.statusAgeMs >= 1000
                    }
                    onClick={() =>
                      void run(async () => {
                        const r = await api(`${endpoint}/push`, {
                          profile: draft!.id,
                          revision: draft!.revision,
                          target: p.number,
                          identity: p.configurationIdentity,
                        });
                        if (r.outcome !== "sent") throw Error(r.message);
                        setNotice(
                          `“${draft!.name}” sent to CDJ ${p.number}. Check the player to confirm application.`,
                        );
                        await refresh();
                      })
                    }
                  >
                    Push {draft ? `“${draft.name}”` : "profile"} to CDJ{" "}
                    {p.number}
                  </button>
                  {p.name !== "CDJ-2000nexus" && (
                    <small>
                      Remote settings for this model are not validated.
                    </small>
                  )}
                </div>
              ))}
              <h3>Read saved settings from a player</h3>
              <p>
                To capture current choices, save MY SETTINGS on the CDJ to its
                USB or SD card first. This reads that saved file; it does not
                read the player’s live configuration directly.
              </p>
              <div className="cdj-config-toolbar">
                <label>
                  Source player
                  <select
                    value={playerId}
                    disabled={busy}
                    onChange={(e) => setPlayerId(e.target.value)}
                  >
                    <option value="">Choose a player</option>
                    {state.players.map((p) => (
                      <option
                        key={p.configurationIdentity}
                        value={p.configurationIdentity}
                      >
                        CDJ {p.number} · {p.ip}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  Media
                  <select
                    value={slot}
                    disabled={busy}
                    onChange={(e) => setSlot(e.target.value)}
                  >
                    <option value="usb">
                      USB
                      {player && !player.configurationUsb
                        ? " — unavailable"
                        : ""}
                    </option>
                    <option value="sd">
                      SD
                      {player && !player.configurationSd
                        ? " — unavailable"
                        : ""}
                    </option>
                  </select>
                </label>
                <button
                  disabled={
                    busy ||
                    dirty ||
                    !player ||
                    player.connection !== "connected" ||
                    !(slot === "usb"
                      ? player.configurationUsb
                      : player.configurationSd)
                  }
                  onClick={() =>
                    void run(async () => {
                      const r = await api(`${endpoint}/import-player`, {
                        target: player!.number,
                        identity: player!.configurationIdentity,
                        slot,
                        name: `CDJ ${player!.number} saved settings`,
                      });
                      await selectImported(r);
                    })
                  }
                >
                  Read saved My Settings
                </button>
              </div>
            </div>
            {automatic && (
              <div className="cdj-config-automatic">
                <h3>Automatic application on connection</h3>
                <label className="cdj-config-check">
                  <input
                    type="checkbox"
                    checked={automatic.enabled}
                    onChange={(e) => setAuto({ enabled: e.target.checked })}
                  />
                  Automatically apply a saved profile
                </label>
                <label>
                  Default for compatible players
                  <select
                    value={automatic.defaultProfile ?? ""}
                    onChange={(e) =>
                      setAuto({ defaultProfile: e.target.value || null })
                    }
                  >
                    <option value="">No default profile</option>
                    {state.profiles.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name}
                      </option>
                    ))}
                  </select>
                </label>
                {state.players
                  .filter((p) => p.configurationIdentity)
                  .map((p) => (
                    <label key={p.configurationIdentity}>
                      CDJ {p.number} override
                      <select
                        value={automatic.players[p.configurationIdentity] ?? ""}
                        onChange={(e) => {
                          const players = { ...automatic.players };
                          if (e.target.value)
                            players[p.configurationIdentity] = e.target.value;
                          else delete players[p.configurationIdentity];
                          setAuto({ players });
                        }}
                      >
                        <option value="">Use default</option>
                        {state.profiles.map((profile) => (
                          <option key={profile.id} value={profile.id}>
                            {profile.name}
                          </option>
                        ))}
                      </select>
                    </label>
                  ))}
                {Object.keys(automatic.players)
                  .filter(
                    (id) =>
                      !state.players.some(
                        (p) => p.configurationIdentity === id,
                      ),
                  )
                  .map((id) => (
                    <div key={id} className="cdj-config-toolbar">
                      <small>Saved disconnected player · {id}</small>
                      <button
                        onClick={() => {
                          const players = { ...automatic.players };
                          delete players[id];
                          setAuto({ players });
                        }}
                      >
                        Remove rule
                      </button>
                    </div>
                  ))}
                <p>
                  Applies once per connection, including when OLC starts. The
                  player must have fresh stopped status; playing players are
                  skipped. A disconnect must last at least 10 seconds to count
                  as a new connection. Saving these rules does not push to
                  players already connected.
                </p>
                <button
                  disabled={busy || !autoDirty || !!state.error}
                  onClick={() =>
                    void run(async () => {
                      await mutate({
                        action: "automatic",
                        automatic,
                        revision: autoRevision,
                      });
                      setAutoDirty(false);
                      setNotice("Automatic connection rules saved.");
                    })
                  }
                >
                  Save automatic settings
                </button>
                <button
                  disabled={busy || !autoDirty}
                  onClick={() => {
                    setAutoDirty(false);
                    setError("");
                  }}
                >
                  Discard rule edits
                </button>
              </div>
            )}
            <div className="cdj-config-log">
              <h3>Recent commands</h3>
              <p>
                Sent means the packet was transmitted; confirm application on
                the CDJ.
              </p>
              {state.events.length === 0 ? (
                <p>No configuration commands sent since this host started.</p>
              ) : (
                state.events
                  .slice(-8)
                  .reverse()
                  .map((e, i) => (
                    <p key={`${e.at}-${i}`}>
                      <time>{new Date(e.at).toLocaleTimeString()}</time> ·{" "}
                      {e.detail.automatic ? "Automatic" : "Manual"} · CDJ{" "}
                      {e.detail.target ?? "?"} · {e.detail.outcome} —{" "}
                      {e.detail.message}
                    </p>
                  ))
              )}
            </div>
          </>
        )}
      </div>
    </section>
  );
}
