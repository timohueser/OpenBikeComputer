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
  return result;
}
