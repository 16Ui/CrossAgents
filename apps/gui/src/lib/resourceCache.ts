/** One connection's shared resource. Invalidations coalesce into one follow-up read. */
export class ResourceCache<T> {
  state: { data?: T; error?: string; loading: boolean } = { loading: true };
  private listeners = new Set<() => void>();
  private epoch = 0;
  private flight?: Promise<void>;
  private dirty = false;
  private readAt = 0;

  private load: () => Promise<T>;
  private maxAgeMs: number;
  constructor(load: () => Promise<T>, maxAgeMs = 30_000) {
    this.load = load;
    this.maxAgeMs = maxAgeMs;
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  snapshot = () => this.state;
  private emit() { this.listeners.forEach((listener) => listener()); }

  /** A different connection must never receive the old connection's response or list. */
  reset = () => {
    this.epoch++;
    this.flight = undefined;
    this.dirty = false;
    this.state = { loading: true };
    this.emit();
  };

  refresh = (): Promise<void> => {
    if (this.flight) {
      this.dirty = true;
      return this.flight;
    }
    const epoch = this.epoch;
    this.flight = Promise.resolve().then(this.load).then(
      (data) => {
        if (epoch === this.epoch) {
          this.readAt = Date.now();
          this.state = { data, loading: false };
        }
      },
      (error) => { if (epoch === this.epoch) this.state = { ...this.state, error: String(error), loading: false }; },
    ).then(() => {
      if (epoch !== this.epoch) return;
      this.flight = undefined;
      this.emit();
      if (this.dirty) {
        this.dirty = false;
        void this.refresh();
      }
    });
    return this.flight;
  };

  /** Mounts share the current request and keep a usable list when a page returns. */
  ensure = () => {
    if (!this.flight && (this.state.data === undefined || Date.now() - this.readAt >= this.maxAgeMs)) void this.refresh();
  };
}
