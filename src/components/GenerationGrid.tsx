import type { SavedResult } from "../app/generation";
import { m, formatTime } from "../i18n";
import { modelLabel } from "../labels";
import { modelByAnyId } from "../models/catalog";

export default function GenerationGrid(p: {
  attempts: SavedResult[];
  selected: SavedResult | null;
  onSelect: (attempt: SavedResult, index: number) => void;
  onEdit: (attempt: SavedResult, index: number) => void;
}) {
  return (
    <div className="generation-grid-scroll">
      {!p.attempts.length && (
        <div className="gallery-empty">
          <strong>{m.grid_empty_title()}</strong>
          <p>{m.grid_empty_help()}</p>
        </div>
      )}
      {[...p.attempts].reverse().map((attempt) => (
        <section className="generation-batch" key={attempt.item.id}>
          <div className="generation-batch-heading">
            <strong>
              {modelLabel(
                attempt.out.model,
                modelByAnyId(attempt.out.model)?.label ?? attempt.out.model,
              )}
            </strong>
            <span>
              {m.count_images({ count: attempt.out.images.length })}
              {attempt.item.status === "running"
                ? attempt.item.batch?.stopped
                  ? m.grid_waiting()
                  : m.grid_continuing()
                : attempt.item.batch?.stopped
                  ? m.grid_stopped()
                  : attempt.item.status === "partial"
                    ? m.grid_partial()
                    : ""}
            </span>
          </div>
          <p title={attempt.snapshot.prompt}>{attempt.snapshot.prompt}</p>
          <div className="generation-grid">
            {attempt.out.images.map((im, index) => {
              const active =
                attempt.item.id === p.selected?.item.id &&
                index === p.selected.selectedIndex;
              const ready = Boolean(im.dataUrl);
              return (
                <div className="generation-cell" key={index}>
                  <button
                    className={active ? "active" : ""}
                    onClick={() => p.onSelect(attempt, index)}
                    aria-pressed={active}
                    aria-label={
                      attempt.item.id === p.selected?.item.id
                        ? m.grid_view({ index: index + 1 })
                        : m.grid_view_batch({
                            time: formatTime(attempt.item.createdAt),
                            index: index + 1,
                          })
                    }
                  >
                    <img
                      src={im.dataUrl}
                      alt={m.grid_candidate({ index: index + 1 })}
                      loading="lazy"
                    />
                    <span>{index + 1}</span>
                  </button>
                  <button
                    className="generation-edit"
                    disabled={!ready}
                    onClick={() => p.onEdit(attempt, index)}
                    aria-label={m.grid_edit_named()}
                  >
                    {m.grid_edit()}
                  </button>
                </div>
              );
            })}
          </div>
        </section>
      ))}
    </div>
  );
}
