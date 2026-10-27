// A modal can be nested in the page or rendered through a portal. Isolate
// siblings along its complete ancestor path, without hiding the modal itself.
const dialogs = new Set();
const previous = new Map();
let observer;

function restore() {
  for (const [element, attributes] of previous) {
    for (const [name, value] of Object.entries(attributes)) {
      if (value === null) element.removeAttribute(name);
      else element.setAttribute(name, value);
    }
  }
  previous.clear();
}

function refresh() {
  restore();
  const top = [
    ...document.querySelectorAll('[role="dialog"][aria-modal="true"]'),
  ]
    .filter((element) => dialogs.has(element) && element.isConnected)
    .at(-1);
  if (!top) return;
  for (
    let current = top;
    current?.parentElement && current !== document.body;
    current = current.parentElement
  ) {
    for (const sibling of current.parentElement.children) {
      if (sibling === current) continue;
      previous.set(sibling, {
        inert: sibling.getAttribute("inert"),
        "aria-hidden": sibling.getAttribute("aria-hidden"),
      });
      // Native inert also prevents focus and interaction. Apply it before
      // aria-hidden so an old focused opener is not hidden while still focused.
      sibling.setAttribute("inert", "");
      sibling.setAttribute("aria-hidden", "true");
    }
  }
  // A containing dialog must not also announce itself as modal while its
  // nested dialog owns interaction. Keep its original role and restore the
  // modal attribute when it becomes the active dialog again.
  for (const dialog of dialogs) {
    if (dialog === top) continue;
    previous.set(dialog, {
      ...previous.get(dialog),
      "aria-modal": dialog.getAttribute("aria-modal"),
    });
    dialog.setAttribute("aria-modal", "false");
  }
}

export function isolateModal(element) {
  dialogs.add(element);
  if (!observer) {
    observer = new MutationObserver(refresh);
    // Background UI can mount after the modal opens. Attribute writes above
    // do not trigger this observer; only actual DOM child changes do.
    observer.observe(document.body, { childList: true, subtree: true });
  }
  refresh();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    dialogs.delete(element);
    refresh();
    if (!dialogs.size) {
      observer.disconnect();
      observer = undefined;
    }
  };
}
