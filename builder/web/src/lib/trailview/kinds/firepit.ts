// Fireplaces: a ring of stones whose fire burns at night, a model on the surface at each point.
import { firepit } from '../models/amenities';
import { modelKind } from './kind';

export default modelKind('firepit', firepit, 800, { light: { colour: '#ff9a4a', radius: 6, at: [0, 0, 0.7] } });
