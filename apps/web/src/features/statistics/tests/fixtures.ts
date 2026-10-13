import type { Job } from '@metrix-writer/contracts';
import type { Recording } from '../../../recorder';
import type { ProcessingAttempt } from '../../modes/api';
const now = new Date(); now.setHours(12, 0, 0, 0);
export function recording(id: string, days = 0): Recording {
 const created = new Date(now); created.setDate(created.getDate() - days);
 return { id, createdAt: created, duration: 60_000, bytes: 100, language: 'en', url: '', filename: 'fixture.wav', downloaded: false,
 metadata: {id, created_at_ms: created.getTime(), duration_ms: 60_000, byte_length: 100, input_language:'en', mime_type:'audio/wav',capture_state:'complete'} };
}
export function job(id: string, recordingId: string, text = 'One two three four', ms = 1000): Job {
 return {id, created_at_ms: now.getTime(), updated_at_ms: now.getTime() + ms,
 request:{recording_id:recordingId,idempotency_key:id,options:{selection:{provider:'local',model:'whisper-test'},input_language:'en',output_language:{mode:'preserve'},vocabulary:{hints:[],corrections:[],snippets:[]},cleanup:{mode:'off'}}},
 status:{state:'succeeded',result:{transcript:{raw_text:text,selection:{provider:'local',model:'whisper-test'},input_language:'en',output_language:{mode:'preserve'},execution_location:'self_hosted_server'},cleanup:{state:'off'}}}};
}
const a = job('job-a','a');
a.audio_preparation = {input_bytes: 42_000_000, output_bytes: 4_000_000, input_ms: 1_312_500, output_ms: 125_000, removed_ms: 1_187_500, preparation_ms: 70};
export const failed: Job = {...job('retry-a','a','',3000),status:{state:'failed',error:{code:'provider_unavailable',message:'Synthetic failure',retryable:true}}};
export const recordings = [recording('a'),recording('b',2),recording('c',4),recording('d',40)];
export const saved = {a:{job:failed,history:[a,a]},b:{job:job('job-b','b','שלום עולם',2000)},c:{job:{...failed,id:'job-c',request:{...failed.request,recording_id:'c'}}},d:{job:job('job-d','d','Older saved words',5000)}};
export function edit(id: string, jobId: string, ms?: number, text_model: ProcessingAttempt['profile']['text_model'] = {provider:'gemini',model:'flash-test'}, served_by?: string, fallback_used = false): ProcessingAttempt {
 return {id,job_id:jobId,created_at_ms:now.getTime(),profile:{id:'super',name:'Super',revision:1,kind:'super',instructions:'Polish',text_model,context:{application:false,clipboard:false,selected_text:false}},request:{operation_id:id,profile_id:'super',profile_revision:1,context:{application:'',selected_text:'',clipboard:''}},raw_text:'Synthetic words',input_text:'Synthetic words',prompt_version:1,state:{state:'succeeded',text:'Synthetic polished text'},
 ...(ms === undefined ? {} : {diagnostics:{processing_ms:ms,input_tokens:100,output_tokens:20,reused_draft:ms===200,first_response_ms:100,response_complete_ms:ms,served_by,fallback_used}})};
}
export const edits = {'job-a':[edit('edit-a','job-a',800),edit('manual-a','job-a',200)],'job-b':[edit('old-b','job-b')],'job-d':[]};
