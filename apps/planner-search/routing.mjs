const variants = {
  balanced: '',
  shortest: '/shorter',
  least_climbing: '/less-climbing',
};
export function routeRequest(input) {
  if (
    !input ||
    !['road', 'gravel', 'mtb', 'touring'].includes(input.bike) ||
    !Array.isArray(input.points) ||
    input.points.length < 2 ||
    input.points.length > 50 ||
    input.points.some(
      (p) =>
        !Array.isArray(p) ||
        p.length !== 2 ||
        !p.every(Number.isFinite) ||
        Math.abs(p[0]) > 180 ||
        Math.abs(p[1]) > 85,
    )
  )
    throw new Error('Invalid route points.');
  if (!Object.hasOwn(variants, input.goal))
    throw new Error(
      `This routing package has no “${input.goal === 'least_unpaved' ? 'least unpaved' : 'most climbing'}” profile. Choose balanced, shorter, or less climbing.`,
    );
  return { points:input.points, profile:input.bike + variants[input.goal], alternatives:false };
}

export async function routeQuery(input) {
  const request = routeRequest(input);
  const base = process.env.OBC_QUERY_ROUTER || 'http://127.0.0.1:8788';
  let response;
  try {
    response = await fetch(`${base}/v1/route`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      signal: AbortSignal.timeout(20000),
      body: JSON.stringify(request),
    });
  } catch {
    throw new Error(
      'Start the local routing engine to apply route requests. Place search and day edits are available now.',
    );
  }
  const result = await response.json();
  if (!response.ok)
    throw new Error(
      result.message || 'The local routing engine could not find a route.',
    );
  return routeReply(input,result);
}

export function routeReply(input,result) {
  routeRequest(input);
  const route = result.routes?.[0];
  if (
    !route ||
    !Array.isArray(route.geometry) ||
    !Array.isArray(route.legs) || route.legs.length !== input.points.length - 1 ||
    route.legs.some((l,i)=>!Number.isInteger(l.from_index)||!Number.isInteger(l.to_index)||l.from_index<0||l.to_index>=route.geometry.length||l.to_index<=l.from_index||l.from_index!==(i?route.legs[i-1].to_index:0))
  )
    throw new Error('The routing engine returned incomplete geometry.');
  const warnings = [];
  if (route.snap_truncated)
    warnings.push('The routing engine reached its snapping search limit.');
  const legs = route.legs.map((l, i) => {
    const line = route.geometry.slice(l.from_index, l.to_index + 1);
    if (line.length < 2) return line;
    for (const [point, end] of [
      [input.points[i], 0],
      [input.points[i + 1], 1],
    ]) {
      const vertex = end ? line.at(-1) : line[0];
      if (point[0] !== vertex[0] || point[1] !== vertex[1]) {
        if (end) line.push(point);
        else line.unshift(point);
        warnings.push(
          'Connections from the selected places to snapped roads are straight and unverified.',
        );
      }
    }
    return line;
  });
  if (
    legs.some(
      (l) =>
        l.length < 2 ||
        l.some(
          (p) =>
            !Array.isArray(p) || p.length !== 2 || !p.every(Number.isFinite),
        ),
    )
  )
    throw new Error('The routing engine returned invalid coordinates.');
  return { legs, warnings: [...new Set(warnings)], totals: route.totals };
}
