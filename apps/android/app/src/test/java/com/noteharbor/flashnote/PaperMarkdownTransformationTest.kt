package com.noteharbor.flashnote

import androidx.compose.ui.text.AnnotatedString
import org.junit.Assert.*
import org.junit.Test

class PaperMarkdownTransformationTest {
 @Test fun formattedTextKeepsAllEditableOffsetsValid() {
  val examples=listOf("", "**", "## 标题\n\n**重点**与*斜体*\n- [ ] 待办\n- [x] 完成", "中文🌿\n[素材](attachment:abc?kind=media&depth=0)","**粗体**")
  for(raw in examples) {
   val result=PaperMarkdownTransformation().filter(AnnotatedString(raw));val map=result.offsetMapping
   val forward=(0..raw.length).map {map.originalToTransformed(it)}
   val backward=(0..result.text.length).map {map.transformedToOriginal(it)}
   assertTrue(forward.all {it in 0..result.text.length})
   assertTrue(backward.all {it in 0..raw.length})
   assertTrue(forward.zipWithNext().all {(a,b)->a<=b})
   assertTrue(backward.zipWithNext().all {(a,b)->a<=b})
   backward.forEachIndexed {display,original -> assertEquals(display,map.originalToTransformed(original))}
  }
 }
 @Test fun markupIsStyledWithoutChangingStoredContent() {
  val raw=AnnotatedString("## 标题\n**重点**与*斜体*\n- [ ] 待办")
  val rendered=PaperMarkdownTransformation().filter(raw)
  assertEquals("标题\n重点与斜体\n□ 待办",rendered.text.text)
  assertEquals(3,rendered.text.spanStyles.size)
  assertTrue(raw.text.contains("**重点**"))
 }
 @Test fun attachmentTokensStayInSourceButNotOnScreen() {
  val raw="正文\n[素材](attachment:abc?kind=media&depth=0)"
  val rendered=PaperMarkdownTransformation().filter(AnnotatedString(raw))
  assertEquals("正文\n",rendered.text.text)
  assertEquals(raw.length,rendered.offsetMapping.transformedToOriginal(rendered.text.length))
 }
}
