/**
 * Marks a branch as unreachable, so a `switch` that misses a case of a union
 * fails to compile (the leftover case isn't assignable to `never`).
 */
export function assertNever(value: never): never {
    throw new Error(`Unexpected value: ${String(value)}`);
}
