import {test} from 'node:test';
import assert from 'node:assert/strict';
import {requiredCases,passedCase,passedApplicationJourney} from './verify-native-candidate.mjs';

test('required native results reject absent, ignored, failed and borrowed cases',()=>{
  const name=requiredCases[0],stdout=`running 1 test\ntest ${name} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1000 filtered out;\n`;
  assert.equal(requiredCases.length,5);assert.equal(passedCase(name,{status:0,stdout}),true);
  for(const result of [{status:1,stdout},{status:null,stdout},{status:0,stdout:stdout.replace('0 ignored','1 ignored')},
    {status:0,stdout:stdout.replace('1 passed','0 passed')},{status:0,stdout:stdout.replace(name,requiredCases[1])}])assert.equal(passedCase(name,result),false);
});

test('the application journey must restore native checkpoint history at the selected canonical base',()=>{
  const base={commit:'a'.repeat(40)},report={nativeRoundtrip:true,nativeApplicationHost:true,checkpointHistoryRestored:true,
    unchangedOperationCount:0,editedOperationCount:1,undoOperationCount:0,embeddedRendererObserved:false,canonicalBase:base};
  const result={status:0,stdout:JSON.stringify(report)};assert.equal(passedApplicationJourney(result,base),true);
  for(const changed of [{...report,checkpointHistoryRestored:false},{...report,editedOperationCount:0},{...report,undoOperationCount:1},
    {...report,canonicalBase:{commit:'b'.repeat(40)}}])assert.equal(passedApplicationJourney({status:0,stdout:JSON.stringify(changed)},base),false);
  assert.equal(passedApplicationJourney({...result,status:1},base),false);
});
