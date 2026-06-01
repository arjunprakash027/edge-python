/* Factory consumed by createWorker({ mainThreadModules: { time } }). Composes the clock + fmt slices. */

import clock from './main/clock.js';
import fmt from './main/fmt.js';

export const time = () => Object.assign({}, clock(), fmt());

export default time;
