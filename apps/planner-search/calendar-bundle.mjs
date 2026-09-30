import {build} from 'esbuild';
import {writeFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';

export async function calendarBundle(inputs = () => {}) {
  const bundle = async (file, name) => {
    const result = await build({
      entryPoints: [fileURLToPath(new URL(file, import.meta.url))], bundle: true,
      format: 'iife', globalName: name, target: 'safari17', write: false, metafile: true,
    });
    inputs(Object.keys(result.metafile.inputs));
    return result.outputFiles[0].text;
  };
  const [dates, hours] = await Promise.all([
    bundle('./calendar-date.mjs', 'Dates'), bundle('./hours.mjs', 'Hours'),
  ]);
  // Only the evaluator and its dependencies see the regional Date constructor.
  return `(Date => {
${dates}
const evaluator = Date => { ${hours}; return Hours; };
globalThis.PlannerCalendar = {calendarDate:Dates.calendarDate,openingHours(config) {
  return evaluator(Dates.calendarDate(config.timeZone)).openingHours(config);
}};
})(globalThis.Date);`;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (!process.argv[2]) throw new Error('Usage: node calendar-bundle.mjs OUTPUT.js');
  writeFileSync(process.argv[2], await calendarBundle());
}
