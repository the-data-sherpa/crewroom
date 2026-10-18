use super::*;
use crate::store::{id, strict, trimmed_text};

impl Store {
    /// The saved Mail message is the delivery intent. The runtime sends that
    /// exact intent through the normal external transport and receipt protocol.
    pub fn prepare_pair_exchange(
        &self,
        attempt_id: &str,
        key: &str,
        input: &Value,
        source: &Value,
    ) -> Result<Value> {
        strict(
            input,
            &[
                "revision",
                "packageId",
                "kind",
                "to",
                "challengeId",
                "responseId",
                "body",
                "artifact",
                "proposedChange",
            ],
        )?;
        text(&json!({"key":key}), "key", 1, 155)?;
        let package = text(input, "packageId", 1, 64)?;
        let kind = text(input, "kind", 1, 20)?;
        trimmed_text(input, "body", 1, 8000)?;
        if !["approach", "challenge", "response", "resolution"].contains(&kind) {
            return Err(conflict("Unknown pair exchange kind."));
        }
        trimmed_text(input, "artifact", 1, 2000)?;
        trimmed_text(input, "proposedChange", 1, 4000)?;
        let receipt = format!("exchange:{attempt_id}:{key}");
        self.transaction(|s| {
            let payload=json!({"attemptId":attempt_id,"input":input});
            if let Some(prior)=s.replay(&receipt,"pair-exchange",&payload)? {return Ok(prior);}
            let (run,assignment,attempt)=s.choreography_caller(attempt_id)?;
            let run_id=text(&run,"id",1,100)?;
            let mut state=s.current_choreography(&run,&input["revision"])?;
            let current=package_mut(&mut state,package)?.clone();
            if !["active","blocked"].contains(&current["state"].as_str().unwrap_or("")) {return Err(conflict("Pair exchanges require an active package."));}
            let owners=array(&state["schedule"]["assignments"]);
            let owner=|responsibility:&str|->Result<&Value> {
                owners.iter().find(|m|m["responsibility"]==responsibility && array(&m["packages"]).contains(&json!(package)))
                    .ok_or_else(||conflict("Package pair is missing."))
            };
            let writer=owner("writer")?;
            let navigator=owner("navigator")?;
            let assignments=s.run_records("assignments",run_id)?;
            let to=assignments.iter().find(|a|(a["name"]==input["to"]||a["id"]==input["to"])&&a["retiredAt"].is_null())
                .ok_or_else(||conflict("Exchange recipient is not admitted."))?;
            let (from_name,to_name)=match kind {
                "approach"|"response"=>(&writer["name"],&navigator["name"]),
                "challenge"=>(&navigator["name"],&writer["name"]),
                _=>(&assignment["name"],&navigator["name"]),
            };
            if assignment["name"]!=*from_name || to["name"]!=*to_name || kind=="resolution" && assignment["coordinator"]!=true {
                return Err(conflict("Exchange must follow writer → navigator → writer and coordinator resolution."));
            }
            let history:Vec<_>=array(&state["exchanges"]).iter().filter(|e|e["packageId"]==package && e["generation"]==current["generation"]).collect();
            let reference=match kind {
                "challenge"=>history.iter().rev().find(|e|e["kind"]=="approach").copied(),
                "response"=>history.iter().find(|e|e["kind"]=="challenge" && e["id"]==input["challengeId"]).copied(),
                "resolution"=>history.iter().find(|e|e["kind"]=="response" && e["id"]==input["responseId"] && e["challengeId"]==input["challengeId"]).copied(),
                _=>None,
            };
            if kind!="approach" {
                let reference=reference.ok_or_else(||conflict("Exchange needs its prior approach, challenge or response."))?;
                let message=s.get_peer_message(text(reference,"messageId",1,100)?)?;
                if message["disposition"]!="received" {return Err(conflict("Wait for the referenced exchange's runner receipt."));}
                // A response may survive replacement, but a late superseded
                // sender cannot attach new authority to the current attempt.
            }
            let exchange_id=id();
            let message_input=json!({"to":input["to"],"body":format!("Package {package} · {kind}\nExchange: {exchange_id}\nSource: {}\nArtifact: {}\n{}\nProposed change / disposition: {}",source,input["artifact"].as_str().unwrap(),input["body"].as_str().unwrap(),input["proposedChange"].as_str().unwrap())});
            let message=s.prepare_message(attempt_id,&format!("pair:{key}"),&message_input)?;
            let exchange=json!({"id":exchange_id,"packageId":package,"generation":current["generation"],"kind":kind,"attemptId":attempt_id,"assignmentId":assignment["id"],"sessionId":attempt["sessionId"],"messageId":message["id"],"source":source,"artifact":input["artifact"],"body":input["body"],"proposedChange":input["proposedChange"],"challengeId":input["challengeId"],"responseId":input["responseId"],"at":s.time()});
            state["exchanges"].as_array_mut().unwrap().push(exchange.clone());
            s.append_choreography(run_id,&mut state,json!({"kind":"exchange","exchangeId":exchange_id,"attemptId":attempt_id}))?;
            let result=json!({"exchange":exchange,"message":message,"revision":state["revision"]});
            s.receipt(&receipt,"pair-exchange",&payload,&result)?;
            Ok(result)
        })
    }
}
