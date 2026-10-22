/**
 * True when keyboard input is going into something the user is typing in, so
 * single-key shortcuts must stand down. One definition: the three copies this
 * replaces disagreed about <select> and contenteditable, which is how "/"
 * could steal focus from the compose editor.
 */
export function isTyping(target) {
  if (!target) return false;
  const tag = target.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || !!target.isContentEditable;
}
