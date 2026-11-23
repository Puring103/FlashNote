package com.noteharbor.flashnote

import android.app.Application
import org.json.JSONObject
import java.io.File
import java.util.UUID

/** Explicit debug-only visual QA. Never opens, seeds or modifies the real vault. */
internal object DesignFixtures {
 fun seed(app: Application,root: String) {
  check(BuildConfig.DEBUG && File(root).name=="design-preview-v2")
  val marker=File(root,"fixture-ready")
  if(marker.exists()) return
  fun call(request: JSONObject): JSONObject {
   val response=JSONObject(Native.request(root,request.toString()))
   check(response.getBoolean("ok")) {response.optString("error")}
   return response.getJSONObject("data")
  }
  val groupIds=mutableMapOf<String,String>()
  listOf("工作" to 0x245356,"生活" to 0xC39B45,"灵感" to 0xB86543,"学习" to 0x747E49).forEach {(name,color) ->
   val groups=call(JSONObject().put("op","group").put("name",name).put("color",color)).getJSONArray("groups")
   for(i in 0 until groups.length()) {val g=groups.getJSONObject(i);if(g.getString("name")==name) groupIds[name]=g.getString("id")}
  }
  val examples=listOf(
   Triple("生活","周末清单","- [ ] 买一束鲜花\n- [ ] 整理桌面\n- [ ] 留一点时间读书"),
   Triple("灵感","产品灵感","便签可以像纸笺一样轻巧。\n用分组色标和清晰的层级，让记录更有条理。"),
   Triple("工作","今日待办","- [x] 整理项目文档\n- [ ] 回复客户邮件\n- [ ] 准备周会汇报"),
   Triple("生活","午后的书桌","把桌面收拾干净，留一点时间给阅读与记录。\n\n## 今天的小计划\n\n- [x] 整理书桌和笔记\n- [ ] 读完手边的一章\n- [ ] 写下三个新想法")
  )
  examples.forEach {(group,title,source) ->
   val data=call(JSONObject().put("op","save").put("id","").put("title",title).put("source",source).put("group",groupIds[group]))
   if(title=="午后的书桌") {
    val id=data.getString("saved_id");val ns=data.getJSONArray("notes")
    val note=(0 until ns.length()).map {ns.getJSONObject(it).getJSONObject("note")}.first {it.getString("id")==id}
    val token=UUID.randomUUID().toString();val file=File(root,"imports/$token");file.parentFile!!.mkdirs()
    app.assets.open("design-desk.jpg").use {input ->file.outputStream().use {input.copyTo(it)}}
    call(JSONObject().put("op","attach").put("id",id).put("revision",note.getString("revision")).put("token",token).put("name","书桌.jpg"))
   }
  }
  marker.writeText("v2")
 }
}
