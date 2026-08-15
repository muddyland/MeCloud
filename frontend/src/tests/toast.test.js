import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { toasts, toast, dismiss, clearToasts } from '$lib/stores/toast.js';

function drainToasts() {
  toasts.subscribe(() => {})();          // subscribe once to flush
  get(toasts).forEach(t => dismiss(t.id));
}

beforeEach(() => {
  drainToasts();
  vi.useFakeTimers();
});

describe('toast store', () => {
  it('starts empty', () => {
    expect(get(toasts)).toHaveLength(0);
  });

  it('adds a toast', () => {
    toast('Hello', 'success');
    expect(get(toasts)).toHaveLength(1);
    expect(get(toasts)[0].message).toBe('Hello');
    expect(get(toasts)[0].type).toBe('success');
  });

  it('assigns a unique id to each toast', () => {
    toast('First');
    toast('Second');
    const [a, b] = get(toasts);
    expect(a.id).not.toBe(b.id);
  });

  it('defaults type to info', () => {
    toast('Neutral');
    expect(get(toasts)[0].type).toBe('info');
  });

  it('dismisses a toast by id', () => {
    toast('Dismiss me', 'error');
    const id = get(toasts)[0].id;
    dismiss(id);
    expect(get(toasts)).toHaveLength(0);
  });

  it('does not affect other toasts when dismissing one', () => {
    toast('Keep');
    toast('Remove');
    const [keep, remove] = get(toasts);
    dismiss(remove.id);
    const remaining = get(toasts);
    expect(remaining).toHaveLength(1);
    expect(remaining[0].id).toBe(keep.id);
  });

  it('auto-dismisses after duration', () => {
    toast('Bye', 'info', 1000);
    expect(get(toasts)).toHaveLength(1);
    vi.advanceTimersByTime(1001);
    expect(get(toasts)).toHaveLength(0);
  });

  it('does not auto-dismiss before duration elapses', () => {
    toast('Still here', 'info', 3500);
    vi.advanceTimersByTime(3000);
    expect(get(toasts)).toHaveLength(1);
  });

  it('stacks multiple toasts', () => {
    toast('A', 'success');
    toast('B', 'error');
    toast('C', 'info');
    expect(get(toasts)).toHaveLength(3);
  });

  it('cancels the auto-dismiss timer when dismissed by hand', () => {
    // Manual dismissal used to leave the timer running, so it fired later
    // against an id that was already gone.
    const id = toast('Bye', 'info', 1000);
    dismiss(id);
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(2000);
    expect(get(toasts)).toHaveLength(0);
  });

  it('returns the id so callers can dismiss a toast themselves', () => {
    const id = toast('Tracked');
    expect(get(toasts)[0].id).toBe(id);
  });

  it('keeps a toast forever when duration is zero', () => {
    toast('Sticky', 'error', 0);
    vi.advanceTimersByTime(60_000);
    expect(get(toasts)).toHaveLength(1);
  });

  it('clearToasts removes everything and cancels every timer', () => {
    toast('A'); toast('B'); toast('C');
    clearToasts();
    expect(get(toasts)).toHaveLength(0);
    expect(vi.getTimerCount()).toBe(0);
  });
});
