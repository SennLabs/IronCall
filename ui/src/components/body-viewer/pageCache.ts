/** Fetches `count` rows starting at `from`. */
export type PageFetcher<T> = (from: number, count: number) => Promise<T[]>;

/**
 * LRU cache of fixed-size row pages for the large body viewer. Concurrent requests for
 * the same page share one fetch; `onLoad` fires when a page arrives so the view can re-render.
 */
export class PageCache<T> {
  private readonly pages = new Map<number, T[]>();
  private readonly inFlight = new Map<number, Promise<void>>();

  constructor(
    private readonly fetcher: PageFetcher<T>,
    readonly pageSize = 200,
    private readonly maxPages = 64,
    private readonly onLoad: () => void = () => {},
  ) {}

  /** The cached row, or undefined if its page isn't loaded yet. */
  row(index: number): T | undefined {
    const page = this.pages.get(Math.floor(index / this.pageSize));
    return page?.[index % this.pageSize];
  }

  get cachedPages(): number {
    return this.pages.size;
  }

  /** Loads every page overlapping rows `first..=last` that isn't cached yet. */
  ensure(first: number, last: number): Promise<void> {
    const loads: Promise<void>[] = [];
    const firstPage = Math.floor(Math.max(0, first) / this.pageSize);
    const lastPage = Math.floor(Math.max(0, last) / this.pageSize);
    for (let page = firstPage; page <= lastPage; page++) {
      const cached = this.pages.get(page);
      if (cached) {
        // Refresh LRU position.
        this.pages.delete(page);
        this.pages.set(page, cached);
        continue;
      }
      loads.push(this.load(page));
    }
    return Promise.all(loads).then(() => undefined);
  }

  private load(page: number): Promise<void> {
    const existing = this.inFlight.get(page);
    if (existing) return existing;
    const promise = this.fetcher(page * this.pageSize, this.pageSize)
      .then((rows) => {
        this.pages.set(page, rows);
        while (this.pages.size > this.maxPages) {
          const oldest = this.pages.keys().next().value;
          if (oldest === undefined) break;
          this.pages.delete(oldest);
        }
        this.onLoad();
      })
      .finally(() => this.inFlight.delete(page));
    this.inFlight.set(page, promise);
    return promise;
  }
}
