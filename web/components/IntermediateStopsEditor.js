import { useState } from "react";
export function IntermediateStopsEditor({ initial = [] }) {
  const [stops, setStops] = useState(initial);
  const change = (id, field, value) =>
    setStops(
      stops.map((s) =>
        s.id !== id
          ? s
          : {
              ...s,
              ...(field === "kind"
                ? { kind: value }
                : { stop: { ...s.stop, [field]: value } }),
            },
      ),
    );
  const move = (at, delta) => {
    const next = stops.slice();
    [next[at], next[at + delta]] = [next[at + delta], next[at]];
    setStops(next);
  };
  return (
    <section className="route-editor" aria-label="Intermediate route stops">
      <input
        type="hidden"
        name="intermediate_stops"
        value={JSON.stringify(stops)}
      />
      <div className="charge-heading">
        <h3>Intermediate stops</h3>
        <button
          className="secondary"
          type="button"
          disabled={stops.length >= 18}
          onClick={() =>
            setStops([
              ...stops,
              {
                id: `stop_${crypto.randomUUID()}`,
                kind: "delivery",
                stop: {
                  facility: "",
                  city: "",
                  state: "",
                  date: "",
                  address_lines: [],
                  postal_code: "",
                  country: "US",
                  contact: "",
                  phone: "",
                  instructions: "",
                },
              },
            ])
          }
        >
          Add intermediate stop
        </button>
      </div>
      <p className="section-note">
        Stops run between origin pickup and final delivery, in this order.
        Full-route carrier agreement and a service confirmation at each stop are
        required. Coordinate these routes through the carrier portal.
      </p>
      {stops.map((s, i) => (
        <fieldset className="route-editor-stop" key={s.id}>
          <legend>Stop {i + 2}</legend>
          <div className="form-grid">
            <label className="field">
              <span>Stop {i + 2} service</span>
              <select
                aria-label={`Stop ${i + 2} service`}
                value={s.kind}
                onChange={(e) => change(s.id, "kind", e.target.value)}
              >
                <option value="pickup">Additional pickup</option>
                <option value="delivery">Intermediate delivery</option>
              </select>
            </label>
            <label className="field">
              <span>Stop {i + 2} facility</span>
              <input
                required
                maxLength={60}
                value={s.stop.facility}
                onChange={(e) => change(s.id, "facility", e.target.value)}
              />
            </label>
          </div>
          <label className="field">
            <span>Stop {i + 2} street address</span>
            <input
              required
              maxLength={55}
              value={s.stop.address_lines?.[0] || ""}
              onChange={(e) =>
                change(s.id, "address_lines", [
                  e.target.value,
                  ...(s.stop.address_lines || []).slice(1),
                ])
              }
            />
          </label>
          <div className="form-grid three">
            {[
              ["city", "City", 30],
              ["state", "State/province", 2],
              ["postal_code", "Postal code", 15],
            ].map(([key, label, max]) => (
              <label className="field" key={key}>
                <span>
                  Stop {i + 2} {label.toLowerCase()}
                </span>
                <input
                  required
                  maxLength={max}
                  value={s.stop[key]}
                  onChange={(e) =>
                    change(
                      s.id,
                      key,
                      key === "state"
                        ? e.target.value.toUpperCase()
                        : e.target.value,
                    )
                  }
                />
              </label>
            ))}
          </div>
          <div className="form-grid">
            <label className="field">
              <span>Stop {i + 2} planned date</span>
              <input
                required
                type="date"
                value={s.stop.date}
                onChange={(e) => change(s.id, "date", e.target.value)}
              />
            </label>
            <label className="field">
              <span>Stop {i + 2} country</span>
              <input
                required
                maxLength={2}
                value={s.stop.country || "US"}
                onChange={(e) =>
                  change(s.id, "country", e.target.value.toUpperCase())
                }
              />
            </label>
          </div>
          <div className="form-grid">
            {[
              ["contact", "contact", 60],
              ["phone", "phone", 25],
            ].map(([key, label, max]) => (
              <label className="field" key={key}>
                <span>
                  Stop {i + 2} {label}
                </span>
                <input
                  maxLength={max}
                  value={s.stop[key] || ""}
                  onChange={(e) => change(s.id, key, e.target.value)}
                />
              </label>
            ))}
          </div>
          <label className="field">
            <span>Stop {i + 2} instructions</span>
            <textarea
              maxLength={1000}
              rows={2}
              value={s.stop.instructions || ""}
              onChange={(e) => change(s.id, "instructions", e.target.value)}
            />
          </label>
          <div className="charge-actions">
            <button
              type="button"
              className="secondary"
              disabled={!i}
              onClick={() => move(i, -1)}
            >
              Move stop {i + 2} earlier
            </button>
            <button
              type="button"
              className="secondary"
              disabled={i === stops.length - 1}
              onClick={() => move(i, 1)}
            >
              Move stop {i + 2} later
            </button>
            <button
              type="button"
              className="text-link"
              onClick={() => setStops(stops.filter((v) => v.id !== s.id))}
            >
              Remove stop {i + 2}
            </button>
          </div>
        </fieldset>
      ))}
    </section>
  );
}
