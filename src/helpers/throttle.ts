import { useEffect, useRef, type DependencyList } from "react";

/**
 * Like useEffect, but runs at most once per `intervalMs`. The first run is
 * immediate; changes inside the window collapse into a single trailing run.
 * The effect receives an AbortSignal that fires when a newer run supersedes it
 * (or on unmount), so async work can discard stale results.
 */
export function useThrottledEffect(
    effect: (signal: AbortSignal) => void | Promise<void>,
    deps: DependencyList,
    intervalMs: number,
) {
    const effectRef = useRef(effect);
    effectRef.current = effect; // trailing run must see the latest closure

    const lastRunRef = useRef<number | null>(null);
    const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const controllerRef = useRef<AbortController | null>(null);

    useEffect(() => {
        const run = () => {
            controllerRef.current?.abort();
            const controller = new AbortController();
            controllerRef.current = controller;
            lastRunRef.current = Date.now();
            timerRef.current = null;
            void effectRef.current(controller.signal);
        };

        if (timerRef.current) clearTimeout(timerRef.current);

        const elapsed = lastRunRef.current === null
            ? Infinity
            : Date.now() - lastRunRef.current;

        if (elapsed >= intervalMs) {
            run();
        } else {
            timerRef.current = setTimeout(run, intervalMs - elapsed);
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [...deps, intervalMs]);

    // Only on unmount: deps changes must not cancel a pending trailing run.
    useEffect(() => () => {
        if (timerRef.current) clearTimeout(timerRef.current);
        controllerRef.current?.abort();
    }, []);
}

export function throttled<T>(fn: () => Promise<T>, intervalMs: number): () => Promise<T> {
    const lastRunRef = { current: null as number | null };
    const run = async (): Promise<T> => {
        const now = Date.now();
        if (lastRunRef.current !== null) {
            const elapsed = now - lastRunRef.current;
            if (elapsed < intervalMs) {
                await new Promise(resolve => setTimeout(resolve, intervalMs - elapsed));
            }
        }
        lastRunRef.current = Date.now();
        return fn();
    };
    return run;
}

export function useTimeout(callback: () => void, delay: number) {
    const savedCallback = useRef(callback);

    useEffect(() => {
        savedCallback.current = callback;
    }, [callback]);

    useEffect(() => {
        if (delay === null) return;
        const id = setTimeout(() => savedCallback.current(), delay);
        return () => clearTimeout(id);
    }, [delay]);
}