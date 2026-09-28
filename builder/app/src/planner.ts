import { mount } from 'svelte';
import './styles/tokens.css';
import PlannerApp from './components/planner/PlannerApp.svelte';

mount(PlannerApp, { target: document.getElementById('app')! });
