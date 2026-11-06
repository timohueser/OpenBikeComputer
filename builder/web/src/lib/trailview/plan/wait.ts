// When the plan worker meshes a cell: apart from the worker, so that the rule tests without one.

/** The cell and its eight neighbours, as `x-y` names. */
export function around(name: string): string[] {
    const [x, y] = name.split('-').map(Number);
    return [-1, 0, 1].flatMap(dy => [-1, 0, 1].map(dx => `${x + dx}-${y + dy}`));
}

/**
 * The cells of `cells` whose mesh may build now: each of their eight neighbours has a rim in `rims`
 * (an empty one where its build or its mesh failed) or is not in `coming`, the cells that the plan will
 * still ask for. A mesh reads the rims that there are and builds again when one of them arrives or
 * changes, so the meshes do not depend on the order of the builds.
 */
export function due(cells: Iterable<string>, rims: ReadonlyMap<string, unknown>, coming: ReadonlySet<string>): string[] {
    return [...cells].filter(name => around(name).every(n => rims.has(n) || !coming.has(n)));
}
