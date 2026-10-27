// Benches, a model on the surface at each point that faces the nearest path.
import { bench } from '../models/amenities';
import { modelKind } from './kind';

export default modelKind('bench', bench, 300, true);
