/**
 * Type guards for data from outside (fetched JSON, messages): what the code reads is checked at run time.
 */

/** An object (not null, not an array): its keys can be read */
export const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

/** An array of numbers */
export const isNumbers = (value: unknown): value is number[] =>
  Array.isArray(value) && value.every((v) => typeof v === 'number');
