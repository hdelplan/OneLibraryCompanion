import { useEffect, useRef, useState, type ReactNode } from "react";

export function SwipeSet({
  title,
  chosen,
  disabled,
  onSelect,
  onDelete,
  children,
}: {
  title: string;
  chosen: boolean;
  disabled: boolean;
  onSelect: () => void;
  onDelete: () => void;
  children: ReactNode;
}) {
  const [distance, setDistance] = useState(0);
  const [confirming, setConfirming] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    if (confirming) dialog.current?.showModal();
    else dialog.current?.close();
  }, [confirming]);
  const close = () => {
    setConfirming(false);
    setDistance(0);
  };
  const gesture = useRef<{
    x: number;
    y: number;
    width: number;
    scale: number;
    horizontal: boolean;
  } | null>(null);
  const moved = useRef(false);
  const confirmDelete = () => {
    if (!disabled) setConfirming(true);
  };
  return (
    <>
      <div
        className="set-swipe"
        style={{ touchAction: "pan-y" }}
        onPointerDown={(e) => {
          if (disabled || e.button !== 0) return;
          moved.current = false;
          gesture.current = {
            x: e.clientX,
            y: e.clientY,
            width: e.currentTarget.clientWidth,
            scale:
              e.currentTarget.clientWidth /
              e.currentTarget.getBoundingClientRect().width,
            horizontal: false,
          };
        }}
        onPointerMove={(e) => {
          const g = gesture.current;
          if (!g) return;
          const dx = g.x - e.clientX,
            dy = Math.abs(g.y - e.clientY);
          if (!g.horizontal && dy > 10 && dy > Math.abs(dx)) {
            gesture.current = null;
            return;
          }
          if (dx > 10 && dx > dy) {
            g.horizontal = true;
            moved.current = true;
            e.currentTarget.setPointerCapture(e.pointerId);
          }
          if (g.horizontal) setDistance(Math.max(0, Math.min(g.width, dx)));
        }}
        onPointerUp={(e) => {
          const g = gesture.current;
          gesture.current = null;
          if (g?.horizontal && (g.x - e.clientX) * g.scale >= g.width * 0.7) {
            setDistance(g.width);
            confirmDelete();
          } else setDistance(0);
        }}
        onPointerCancel={() => {
          gesture.current = null;
          setDistance(0);
        }}
      >
        <button
          className="set-swipe-delete"
          disabled={disabled}
          aria-label={`Delete ${title}`}
          onClick={confirmDelete}
        >
          DELETE
        </button>
        <button
          className={`set-swipe-entry ${chosen ? "chosen" : ""}`}
          disabled={disabled}
          style={{ transform: `translateX(-${distance}px)` }}
          onKeyDown={(e) => {
            if (e.key === "Delete") {
              e.preventDefault();
              confirmDelete();
            }
          }}
          onClick={() => {
            if (!moved.current) onSelect();
            moved.current = false;
          }}
        >
          {children}
        </button>
      </div>
      <dialog
        ref={dialog}
        className="set-delete-dialog"
        aria-label="Delete set"
        onCancel={close}
      >
        <p>The set {title} will be deleted, are you sure?</p>
        <div>
          <button autoFocus onClick={close}>
            Cancel
          </button>
          <button
            className="set-delete-confirm"
            disabled={disabled}
            onClick={() => {
              close();
              onDelete();
            }}
          >
            DELETE
          </button>
        </div>
      </dialog>
    </>
  );
}
