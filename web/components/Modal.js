import { useEffect, useRef, useId } from "react";
import { X } from "lucide-react";
import { isolateModal } from "../lib/modal-isolation.mjs";

let activeModals = 0;
let previousBodyOverflow;

function canFocus(element) {
  if (!element?.isConnected) return false;
  if (
    typeof element.checkVisibility === "function" &&
    !element.checkVisibility()
  )
    return false;
  // Older engines may expose layout rectangles for collapsed details content.
  // Only the first summary's subtree remains visible in a closed details node.
  for (
    let parent = element.parentElement;
    parent;
    parent = parent.parentElement
  ) {
    if (parent.tagName === "DETAILS" && !parent.open) {
      const summary = [...parent.children].find(
        (child) => child.tagName === "SUMMARY",
      );
      if (!summary?.contains(element)) return false;
    }
  }
  return (
    element.tabIndex >= 0 &&
    !element.matches(":disabled") &&
    !element.closest("[inert]") &&
    element.getClientRects().length > 0 &&
    window.getComputedStyle(element).visibility === "visible"
  );
}
function focusTargets(root) {
  return root
    ? [
        ...root.querySelectorAll(
          "button,input,select,textarea,a[href],summary,[tabindex]",
        ),
      ].filter(canFocus)
    : [];
}

export function Modal({ title, subtitle, children, close, wide }) {
  const ref = useRef(null);
  const titleId = useId();
  const closeRef = useRef(close);
  closeRef.current = close;
  useEffect(() => {
    const previous = document.activeElement;
    const releaseIsolation = isolateModal(ref.current);
    focusTargets(ref.current)[0]?.focus();
    const key = (event) => {
      if (
        [...document.querySelectorAll('[role="dialog"][aria-modal="true"]')].at(
          -1,
        ) !== ref.current
      )
        return;
      if (event.key === "Escape") closeRef.current();
      if (event.key === "Tab") {
        const items = focusTargets(ref.current);
        const first = items[0],
          last = items.at(-1);
        const outside = !items.includes(document.activeElement);
        if (!first) {
          event.preventDefault();
          ref.current?.focus();
        } else if (
          event.shiftKey &&
          (document.activeElement === first || outside)
        ) {
          event.preventDefault();
          last?.focus();
        } else if (
          !event.shiftKey &&
          (document.activeElement === last || outside)
        ) {
          event.preventDefault();
          first?.focus();
        }
      }
    };
    document.addEventListener("keydown", key);
    if (activeModals++ === 0) {
      previousBodyOverflow = document.body.style.overflow;
      document.body.style.overflow = "hidden";
    }
    return () => {
      document.removeEventListener("keydown", key);
      releaseIsolation();
      if (--activeModals === 0)
        document.body.style.overflow = previousBodyOverflow;
      if (canFocus(previous)) previous.focus();
      else
        focusTargets(
          [
            ...document.querySelectorAll('[role="dialog"][aria-modal="true"]'),
          ].at(-1),
        )[0]?.focus();
    };
  }, []);
  return (
    <div
      className="overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <section
        ref={ref}
        className={`modal ${wide ? "wide" : ""}`}
        role="dialog"
        tabIndex={-1}
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <div className="modal-header">
          <div>
            <h2 id={titleId}>{title}</h2>
            {subtitle && <p>{subtitle}</p>}
          </div>
          <button
            className="icon-button"
            aria-label="Close dialog"
            onClick={close}
          >
            <X size={20} />
          </button>
        </div>
        {children}
      </section>
    </div>
  );
}
