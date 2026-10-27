// Barbecues: a stone hearth whose fire burns at night, a model on the surface at each point.
import { bbq } from '../models/amenities';
import { modelKind } from './kind';

export default modelKind('bbq', bbq, 800, { light: { colour: '#ff9a4a', radius: 5 } });
