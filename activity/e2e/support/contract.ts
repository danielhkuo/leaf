// Holds the server's answers to the schemas the client reads them with
// (src/lib/api/schemas.ts, src/lib/admin/schemas.ts), key by key.
//
// Parsing alone cannot do this. A field the client reads through `opt()` may
// be absent, so an answer that has lost or renamed one still parses, without
// a word in the console: the detail it fed is just gone from the screen. So
// every key a schema names is looked for in what the server actually sent,
// and every key the server sent is looked for in the schema.

import { z } from 'zod';

/** A schema under its `opt()`, `nullable()` and `default()` wrappers. */
function bare(schema: z.ZodTypeAny): z.ZodTypeAny {
  let inner = schema;
  for (;;) {
    if (inner instanceof z.ZodEffects) inner = inner.innerType() as z.ZodTypeAny;
    else if (inner instanceof z.ZodCatch) inner = inner.removeCatch() as z.ZodTypeAny;
    else if (inner instanceof z.ZodOptional || inner instanceof z.ZodNullable) {
      inner = inner.unwrap() as z.ZodTypeAny;
    } else if (inner instanceof z.ZodDefault) inner = inner.removeDefault() as z.ZodTypeAny;
    else return inner;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** How often a key was there, of the objects of its shape the server sent. */
interface Count {
  objects: number;
  present: number;
}

export class Contract {
  /** By path, e.g. `series[].total_days`: every key a checked schema names. */
  readonly #named = new Map<string, Count>();
  readonly #unread = new Set<string>();
  readonly #unreadable: string[] = [];
  /** Every schema an answer was read with, the ones nested in it included. */
  readonly #used = new Set<z.ZodTypeAny>();

  /**
   * Reads one answer as the client does, with `schema`, and compares its
   * keys with the schema's. Answers checked under one `label` are counted
   * together. `shape` is the schema whose keys are compared, where the
   * client's own cannot be walked (a union).
   */
  check(label: string, schema: z.ZodTypeAny, body: unknown, shape: z.ZodTypeAny = schema): void {
    this.#used.add(schema);
    // `opt()` reports a field it could not read with console.warn and
    // carries on. Here that is a finding, like a failed parse.
    const warnings: string[] = [];
    const warn = console.warn;
    console.warn = (...args: unknown[]) => warnings.push(args.map(String).join(' '));
    try {
      const parsed = schema.safeParse(body);
      if (!parsed.success) {
        const issues = parsed.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`);
        this.#unreadable.push(`${label}: ${issues.join('; ')}`);
      }
    } finally {
      console.warn = warn;
    }
    for (const warning of warnings) this.#unreadable.push(`${label}: ${warning}`);
    this.#name(shape, label);
    this.#compare(shape, body, label);
  }

  /** Lists every key `schema` names, so one no answer reached is still known of. */
  #name(schema: z.ZodTypeAny, path: string): void {
    const inner = bare(schema);
    if (inner instanceof z.ZodArray) this.#name(inner.element as z.ZodTypeAny, `${path}[]`);
    if (!(inner instanceof z.ZodObject)) return;
    for (const [key, field] of Object.entries(inner.shape as z.ZodRawShape)) {
      const at = `${path}.${key}`;
      if (!this.#named.has(at)) this.#named.set(at, { objects: 0, present: 0 });
      this.#name(field, at);
    }
  }

  #compare(schema: z.ZodTypeAny, value: unknown, path: string): void {
    this.#used.add(schema);
    const inner = bare(schema);
    this.#used.add(inner);
    if (inner instanceof z.ZodArray) {
      if (!Array.isArray(value)) return;
      for (const item of value) this.#compare(inner.element as z.ZodTypeAny, item, `${path}[]`);
    } else if (inner instanceof z.ZodObject && isRecord(value)) {
      const shape = inner.shape as z.ZodRawShape;
      for (const [key, field] of Object.entries(shape)) {
        const at = `${path}.${key}`;
        const count = this.#named.get(at);
        if (!count) throw new Error(`${at} was not named before it was compared`);
        count.objects += 1;
        if (!(key in value)) continue;
        count.present += 1;
        this.#compare(field, value[key], at);
      }
      for (const key of Object.keys(value)) {
        if (!(key in shape)) this.#unread.add(`${path}.${key}`);
      }
    }
  }

  /** Answers the client could not read, or read only by dropping a field. */
  unreadable(): string[] {
    return [...this.#unreadable];
  }

  /**
   * Keys a schema names that no answer had: renamed or dropped on one side,
   * or (when no answer held an object of that shape) not sampled at all.
   */
  neverSent(): string[] {
    const never = [...this.#named].filter(([, count]) => count.present === 0);
    return never.map(([path]) => path).sort();
  }

  /** Keys that some answers had and others of the same shape did not. */
  sometimesSent(): string[] {
    return [...this.#named]
      .filter(([, count]) => count.present > 0 && count.present < count.objects)
      .map(([path]) => path)
      .sort();
  }

  /** Keys the server sent that no schema names: the client drops them unread. */
  unread(): string[] {
    return [...this.#unread].sort();
  }

  /** Whether an answer was read with `schema`, directly or as part of another. */
  used(schema: z.ZodTypeAny): boolean {
    return this.#used.has(schema);
  }
}
